//! Phones and cameras (MTP/PTP) on Windows through the Windows Portable Devices COM API —
//! the work GVfs' gphoto2 backend does on Linux. The decision this module embodies: devices
//! are not drive letters (no filesystem driver is installable on this network), they run
//! through the app's own remote-folder machinery, so a storage shows as a `Location::Remote`
//! addressed `wpd://<PnP instance path>/<storage object id>/<folder…>` (see `location`).
//!
//! COM discipline mirrors `win::recyclebin`: a dedicated worker thread owns every WPD object
//! (device proxies are apartment-affine and must never cross threads); the public functions
//! box work up and block until the worker has run it. Open devices are cached there, one
//! `IPortableDevice` per PnP path, invalidated on the first failing call so an unplug and
//! replug reconnects by itself.
//!
//! WPD object ids never contain "/", which is what lets `RemoteAt` pack them into its URI.
//! The API shapes below (batched `EnumObjects`, PROPVARIANT ownership through collections,
//! the values-bag string types, stream read/write) were verified end-to-end against the live
//! COM objects on the build host; the device-facing behaviour still awaits real hardware.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
use std::path::Path;
use std::sync::mpsc;
use std::sync::OnceLock;

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CM_Query_And_Remove_SubTreeW, PNP_VETO_TYPE};
use windows::Win32::Devices::PortableDevices::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IStream};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0, PropVariantClear};
use windows::Win32::System::Variant::{VT_BSTR, VT_DATE, VT_FILETIME, VT_LPWSTR};

// The WPD coclasses and the functional-category GUID, extracted from Microsoft's own
// PortableDeviceGuids.lib (Windows SDK 10.0.26100 um/x64) — the authoritative definitions
// of the objects portabledeviceapi.h names. The SDK header trims IPortableDeviceCapabilities
// to the eleven methods projected by windows 0.62, and that trimmed order matches the
// shipped vtable, so the projection needs no hand-declared slots.
const CLSID_PORTABLE_DEVICE: GUID = GUID::from_u128(0x728A21C5_3D9E_48D7_9810_864848F0F404);
const CLSID_PORTABLE_DEVICE_MANAGER: GUID = GUID::from_u128(0x0AF10CEC_2ECD_4B92_9581_34F6AE0637F3);
const CLSID_PORTABLE_DEVICE_VALUES: GUID = GUID::from_u128(0x0C15D503_D017_47CE_9016_7B3F978721CC);
const CLSID_PORTABLE_DEVICE_KEY_COLLECTION: GUID = GUID::from_u128(0xDE2D022D_2480_43BE_97F0_D1FA2CF98F4F);
const CLSID_PORTABLE_DEVICE_PVC: GUID = GUID::from_u128(0x08A99E2F_6D6D_4B80_AF5A_BAF2BCBE4CB9);
const FUNCTIONAL_CATEGORY_ALL: GUID = GUID::from_u128(0x2D8A6512_A74C_448E_BA8A_F4AC07C49399);

/// One device as the manager sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub pnp: String,
    pub name: String,
}

/// One storage on a device: enough to show capacity and whether writing is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Storage {
    pub id: String,
    pub name: String,
    pub total: u64,
    pub free: u64,
    pub writable: bool,
}

type Job = Box<dyn FnOnce(&mut Ctx) + Send>;

static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

/// The worker's state, alive only on the worker thread: the device manager and the
/// `IPortableDevice` cache. Every WPD interface it holds stays inside this thread.
struct Ctx {
    manager: IPortableDeviceManager,
    devices: HashMap<String, IPortableDevice>,
}

impl Ctx {
    /// Runs `f` against the device for `pnp`, opening (or reopening) it as needed. Any error
    /// drops the cached device: assuming the connection went bad is cheaper than deciding
    /// which refusals mean "gone", and the next call reconnects.
    fn with_device<T>(&mut self, pnp: &str, f: impl FnOnce(&IPortableDevice) -> io::Result<T>) -> io::Result<T> {
        let device = match self.devices.get(pnp) {
            Some(d) => d.clone(),
            None => {
                let d = match unsafe { open_device(pnp) } {
                    Ok(d) => d,
                    Err(e) => return Err(e),
                };
                self.devices.insert(pnp.to_string(), d.clone());
                d
            }
        };
        let outcome = f(&device);
        if outcome.is_err() {
            self.devices.remove(pnp);
        }
        outcome
    }
}

/// The one thread every WPD object lives on.
fn worker() -> &'static mpsc::Sender<Job> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("fb-wpd".to_string())
            .spawn(move || unsafe {
                if CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().is_err() {
                    return;
                }
                let manager: windows::core::Result<IPortableDeviceManager> = CoCreateInstance(&CLSID_PORTABLE_DEVICE_MANAGER, None, CLSCTX_INPROC_SERVER);
                if let Ok(manager) = manager {
                    let mut ctx = Ctx { manager, devices: HashMap::new() };
                    while let Ok(job) = rx.recv() {
                        job(&mut ctx);
                    }
                }
                CoUninitialize();
            })
            .expect("the WPD worker thread cannot fail to start");
        tx
    })
}

fn gone() -> io::Error {
    io::Error::other("the WPD worker thread is gone")
}

/// Hands one unit of work to the worker thread and blocks for its result.
fn call<T: Send + 'static>(f: impl FnOnce(&mut Ctx) -> io::Result<T> + Send + 'static) -> io::Result<T> {
    let (tx, rx) = mpsc::channel();
    worker().send(Box::new(move |ctx| {
        let _ = tx.send(f(ctx));
    }))
        .map_err(|_| gone())?;
    rx.recv().map_err(|_| gone())?
}

/// Maps a WPD failure onto io errors: the "no such object" family becomes `NotFound`
/// (the remote machinery probes existence with it), access refusals become
/// `PermissionDenied`, everything else keeps its raw code for diagnosis.
fn map(e: windows::core::Error) -> io::Error {
    let code = e.code().0;
    match code as u32 {
        0x8003_0003 | 0x8007_0002 | 0x8007_0003 => return io::Error::new(io::ErrorKind::NotFound, e.to_string()),
        0x8007_0005 => return io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()),
        _ => {}
    }
    io::Error::from_raw_os_error(code)
}

// ── values-bag plumbing ─────────────────────────────────────────────────────

unsafe fn new_values() -> io::Result<IPortableDeviceValues> {
    unsafe {
        let bag: windows::core::Result<IPortableDeviceValues> = CoCreateInstance(&CLSID_PORTABLE_DEVICE_VALUES, None, CLSCTX_INPROC_SERVER);
        bag.map_err(map)
    }
}

unsafe fn new_pvc() -> io::Result<IPortableDevicePropVariantCollection> {
    unsafe {
        let coll: windows::core::Result<IPortableDevicePropVariantCollection> = CoCreateInstance(&CLSID_PORTABLE_DEVICE_PVC, None, CLSCTX_INPROC_SERVER);
        coll.map_err(map)
    }
}

unsafe fn keybag(keys: &[&PROPERTYKEY]) -> io::Result<IPortableDeviceKeyCollection> {
    unsafe {
        let bag: windows::core::Result<IPortableDeviceKeyCollection> = CoCreateInstance(&CLSID_PORTABLE_DEVICE_KEY_COLLECTION, None, CLSCTX_INPROC_SERVER);
        let bag = bag.map_err(map)?;
        for k in keys {
            bag.Add(*k).map_err(map)?;
        }
        Ok(bag)
    }
}

/// A `VT_LPWSTR` variant whose wide string lives in CoTaskMem — exactly what WPD
/// collections expect. The collection takes **ownership** on `Add`: freeing the string
/// afterwards was proven (probe, on this host) to double-free when the collection releases.
unsafe fn lpwstr(value: &str) -> PROPVARIANT {
    unsafe {
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let p = CoTaskMemAlloc(wide.len() * 2).cast::<u16>();
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { pwszVal: PWSTR(p) },
                }),
            },
        }
    }
}

unsafe fn free_variant_string(pv: &mut PROPVARIANT) {
    unsafe {
        if pv.Anonymous.Anonymous.vt == VT_LPWSTR {
            CoTaskMemFree(Some(pv.Anonymous.Anonymous.Anonymous.pwszVal.0 as *const _));
        }
    }
}

/// The string a variant carries — WPD hands object ids back as `VT_BSTR` inside
/// collections but `VT_LPWSTR` everywhere else; accept both.
unsafe fn read_variant(pv: &PROPVARIANT) -> String {
    unsafe {
        match pv.Anonymous.Anonymous.vt {
            VT_LPWSTR => pv.Anonymous.Anonymous.Anonymous.pwszVal.to_string().unwrap_or_default(),
            VT_BSTR => format!("{:?}", &*pv.Anonymous.Anonymous.Anonymous.bstrVal).trim_matches('"').to_string(),
            _ => String::new(),
        }
    }
}

unsafe fn ids_of(coll: &IPortableDevicePropVariantCollection) -> io::Result<Vec<String>> {
    unsafe {
        let mut count = 0u32;
        coll.GetCount(&raw mut count).map_err(map)?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut pv = PROPVARIANT::default();
            coll.GetAt(i, &raw mut pv).map_err(map)?;
            out.push(read_variant(&pv));
            PropVariantClear(&raw mut pv).ok();
        }
        Ok(out)
    }
}

unsafe fn wstr(bag: &IPortableDeviceValues, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let raw = bag.GetStringValue(key).ok()?;
        let value = raw.to_string().ok()?;
        CoTaskMemFree(Some(raw.0 as *const _));
        Some(value)
    }
}

/// FILETIME counts 100 ns steps since 1601-01-01: that many steps from the Unix epoch.
const FILETIME_UNIX_DIFF: u64 = 116_444_736_000_000_000;

/// A variant carrying a modification time as epoch seconds: devices spell this as
/// `VT_FILETIME` or `VT_DATE` (OLE days since 1899-12-30); anything else is 0 ("hidden",
/// the same value the SFTP/FTP arms use).
unsafe fn mtime_variant(pv: &PROPVARIANT) -> i64 {
    unsafe {
        match pv.Anonymous.Anonymous.vt {
            VT_FILETIME => {
                let ft = pv.Anonymous.Anonymous.Anonymous.filetime;
                let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
                ticks.saturating_sub(FILETIME_UNIX_DIFF) as i64 / 10_000_000
            }
            VT_DATE => (pv.Anonymous.Anonymous.Anonymous.date * 86_400.0 - 2_209_161_600.0) as i64,
            _ => 0,
        }
    }
}

unsafe fn mtime(bag: &IPortableDeviceValues, key: &PROPERTYKEY) -> i64 {
    unsafe {
        match bag.GetValue(key) {
            Ok(mut pv) => {
                let value = mtime_variant(&pv);
                PropVariantClear(&raw mut pv).ok();
                value
            }
            Err(_) => 0,
        }
    }
}

// ── device and object walks ─────────────────────────────────────────────────

unsafe fn open_device(pnp: &str) -> io::Result<IPortableDevice> {
    unsafe {
        let device: windows::core::Result<IPortableDevice> = CoCreateInstance(&CLSID_PORTABLE_DEVICE, None, CLSCTX_INPROC_SERVER);
        let device = device.map_err(map)?;
        let info = new_values()?;
        info.SetStringValue(&WPD_CLIENT_NAME, &HSTRING::from("failBrauwser")).map_err(map)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1).map_err(map)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0).map_err(map)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0).map_err(map)?;
        // 0 means read/write — the same encoding the storage access capability uses. Not
        // every device grants writes without being asked for it.
        info.SetUnsignedIntegerValue(&WPD_CLIENT_DESIRED_ACCESS, 0).map_err(map)?;
        device.Open(&HSTRING::from(pnp), &info).map_err(map)?;
        Ok(device)
    }
}

/// Enumerates one folder's child object ids (batched; WPD's enumerator hands out its batch
/// and stops with a failure HRESULT past the end).
unsafe fn children(content: &IPortableDeviceContent, parent: &str) -> io::Result<Vec<String>> {
    unsafe {
        let enumerator = content.EnumObjects(0, &HSTRING::from(parent), None::<&IPortableDeviceValues>).map_err(map)?;
        let mut out = Vec::new();
        let mut batch = vec![PWSTR::null(); 64];
        loop {
            let mut fetched = 0u32;
            let hr = enumerator.Next(&mut batch, &raw mut fetched);
            for slot in &mut batch[..fetched as usize] {
                out.push(slot.to_string().unwrap_or_default());
                CoTaskMemFree(Some(slot.0 as *const _));
                *slot = PWSTR::null();
            }
            if hr.is_err() || fetched == 0 {
                return Ok(out);
            }
        }
    }
}

/// Walks `path` ("/"-joined, "" is the storage itself) down from `storage`'s object id,
/// matching names case-insensitively (MTP devices spell names however they like).
unsafe fn resolve(content: &IPortableDeviceContent, storage: &str, path: &str) -> io::Result<String> {
    unsafe {
        let props = content.Properties().map_err(map)?;
        let keys = keybag(&[&WPD_OBJECT_NAME])?;
        let mut current = storage.to_string();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            let wanted = seg.to_lowercase();
            let found = children(content, &current)?
                .into_iter()
                .find(|id| props.GetValues(&HSTRING::from(id), &keys).ok().and_then(|bag| wstr(&bag, &WPD_OBJECT_NAME)).is_some_and(|name| name.to_lowercase() == wanted));
            current = found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("the device has no “{seg}”")))?;
        }
        Ok(current)
    }
}

/// (parent path, leaf name) of a slash path — the storage root is "".
fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((parent, leaf)) => (parent.to_string(), leaf.to_string()),
        None => (String::new(), path.to_string()),
    }
}

// ── the public surface: blocking calls from any job thread ──────────────────

/// The devices WPD currently knows about (names already resolved).
pub fn devices() -> io::Result<Vec<Device>> {
    call(|ctx| unsafe {
        ctx.manager.RefreshDeviceList().map_err(map)?;
        let mut count = 0u32;
        ctx.manager.GetDevices(std::ptr::null_mut(), &raw mut count).map_err(map)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let ids: *mut PWSTR = CoTaskMemAlloc(count as usize * std::mem::size_of::<PWSTR>()).cast();
        if ids.is_null() {
            return Err(io::Error::from_raw_os_error(8));
        }
        let filled = ctx.manager.GetDevices(ids, &raw mut count);
        let mut out = Vec::new();
        if filled.is_ok() {
            for i in 0..count {
                let raw = *ids.add(i as usize);
                let pnp = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.0 as *const _));
                if pnp.is_empty() {
                    continue;
                }
                let mut buf = [0u16; 512];
                let mut len = buf.len() as u32;
                let name = match ctx.manager.GetDeviceFriendlyName(&HSTRING::from(&pnp), PWSTR(buf.as_mut_ptr()), &raw mut len) {
                    Ok(()) => String::from_utf16_lossy(&buf[..len.min(buf.len() as u32) as usize]),
                    Err(_) => pnp.clone(),
                };
                out.push(Device { pnp, name });
            }
        }
        CoTaskMemFree(Some(ids as *const _));
        filled.map_err(map)?;
        Ok(out)
    })
}

/// The storages a device exposes. The object tree is walked, not hard-coded: the object
/// ids of functional objects come from `GetFunctionalObjects(ALL)`, and a child of one is
/// a storage exactly when it answers the storage capacity property.
pub fn storages(pnp: &str) -> io::Result<Vec<Storage>> {
    let pnp = pnp.to_string();
    call(move |ctx| ctx.with_device(&pnp, |device| unsafe {
        let caps = device.Capabilities().map_err(map)?;
        let functional = caps.GetFunctionalObjects(&FUNCTIONAL_CATEGORY_ALL).map_err(map)?;
        let content = device.Content().map_err(map)?;
        let props = content.Properties().map_err(map)?;
        let keys = keybag(&[&WPD_OBJECT_NAME, &WPD_STORAGE_CAPACITY, &WPD_STORAGE_FREE_SPACE_IN_BYTES, &WPD_STORAGE_ACCESS_CAPABILITY])?;
        let mut out = Vec::new();
        for fid in ids_of(&functional)? {
            for id in children(&content, &fid)? {
                let bag = match props.GetValues(&HSTRING::from(&id), &keys) {
                    Ok(bag) => bag,
                    Err(_) => continue, // not a storage: it refuses the capacity key
                };
                let access = bag.GetSignedIntegerValue(&WPD_STORAGE_ACCESS_CAPABILITY).unwrap_or(0);
                out.push(Storage {
                    id,
                    name: wstr(&bag, &WPD_OBJECT_NAME).unwrap_or_else(|| "storage".to_string()),
                    total: bag.GetUnsignedLargeIntegerValue(&WPD_STORAGE_CAPACITY).unwrap_or(0),
                    free: bag.GetUnsignedLargeIntegerValue(&WPD_STORAGE_FREE_SPACE_IN_BYTES).unwrap_or(0),
                    writable: access == WPD_STORAGE_ACCESS_CAPABILITY_READWRITE.0,
                });
            }
        }
        Ok(out)
    }))
}

fn entry_from(bag: &IPortableDeviceValues, id: &str) -> crate::location::RemoteEntry {
    unsafe {
        crate::location::RemoteEntry {
            name: wstr(bag, &WPD_OBJECT_NAME).unwrap_or_else(|| id.to_string()),
            size: bag.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE).unwrap_or(0),
            mtime: mtime(bag, &WPD_OBJECT_DATE_MODIFIED),
            is_dir: bag.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE).map(|g| g == WPD_CONTENT_TYPE_FOLDER).unwrap_or(false),
            is_link: false,
        }
    }
}

/// Lists one folder of one storage ("" is the storage's top).
pub fn list(pnp: &str, storage: &str, path: &str) -> io::Result<Vec<crate::location::RemoteEntry>> {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let props = content.Properties().map_err(map)?;
        let folder = resolve(&content, &storage, &path)?;
        let keys = keybag(&[&WPD_OBJECT_NAME, &WPD_OBJECT_SIZE, &WPD_OBJECT_DATE_MODIFIED, &WPD_OBJECT_CONTENT_TYPE])?;
        let mut out = Vec::new();
        for id in children(&content, &folder)? {
            let bag = props.GetValues(&HSTRING::from(&id), &keys).map_err(map)?;
            out.push(entry_from(&bag, &id));
        }
        Ok(out)
    }))
}

/// Downloads one file into `local_out`.
pub fn read_to(pnp: &str, storage: &str, path: &str, local_out: &Path) -> io::Result<u64> {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    let local_out = local_out.to_path_buf();
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let id = resolve(&content, &storage, &path)?;
        let resources = content.Transfer().map_err(map)?;
        let mut size = 0u32;
        let mut stream: Option<IStream> = None;
        resources.GetStream(&HSTRING::from(&id), &WPD_RESOURCE_DEFAULT, 0, &raw mut size, &raw mut stream).map_err(map)?;
        let stream = stream.ok_or_else(|| io::Error::other("the device offered the file but no stream"))?;
        let mut file = std::fs::File::create(&local_out)?;
        let mut chunk = vec![0u8; size.max(65_536) as usize];
        let mut total = 0u64;
        loop {
            let mut got = 0u32;
            stream.Read(chunk.as_mut_ptr() as *mut core::ffi::c_void, chunk.len() as u32, Some(&raw mut got)).ok().map_err(map)?;
            if got == 0 {
                break;
            }
            file.write_all(&chunk[..got as usize])?;
            total += got as u64;
            if (got as usize) < chunk.len() {
                break;
            }
        }
        Ok(total)
    }))
}

/// Uploads `local_in` as `path` (its parent folder must exist). Releasing the stream is
/// what commits the object — devices size the stored file from the bytes actually written.
pub fn write_from(pnp: &str, storage: &str, local_in: &Path, path: &str) -> io::Result<u64> {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    let local_in = local_in.to_path_buf();
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let (parent, leaf) = split_path(&path);
        let parent_id = resolve(&content, &storage, &parent)?;
        let bag = new_values()?;
        bag.SetStringValue(&WPD_OBJECT_PARENT_ID, &HSTRING::from(&parent_id)).map_err(map)?;
        bag.SetStringValue(&WPD_OBJECT_NAME, &HSTRING::from(&leaf)).map_err(map)?;
        bag.SetGuidValue(&WPD_OBJECT_CONTENT_TYPE, &WPD_CONTENT_TYPE_GENERIC_FILE).map_err(map)?;
        let mut stream: Option<IStream> = None;
        let mut optimal = 0u32;
        let mut cookie = PWSTR::null();
        content.CreateObjectWithPropertiesAndData(&bag, &raw mut stream, &raw mut optimal, &raw mut cookie).map_err(map)?;
        if !cookie.is_null() {
            CoTaskMemFree(Some(cookie.0 as *const _));
        }
        let stream = stream.ok_or_else(|| io::Error::other("the device created the file but offered no stream"))?;
        let mut file = std::fs::File::open(&local_in)?;
        let mut chunk = vec![0u8; optimal.max(65_536) as usize];
        let mut total = 0u64;
        loop {
            let n = file.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            let mut written = 0u32;
            stream.Write(chunk.as_ptr() as *const core::ffi::c_void, n as u32, Some(&raw mut written)).ok().map_err(map)?;
            if written as usize != n {
                return Err(io::Error::other("the device accepted only part of the file"));
            }
            total += written as u64;
        }
        drop(stream);
        Ok(total)
    }))
}

/// Creates a folder; the new object id is not kept.
pub fn mkdir(pnp: &str, storage: &str, path: &str) -> io::Result<()> {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let (parent, leaf) = split_path(&path);
        let parent_id = resolve(&content, &storage, &parent)?;
        let bag = new_values()?;
        bag.SetStringValue(&WPD_OBJECT_PARENT_ID, &HSTRING::from(&parent_id)).map_err(map)?;
        bag.SetStringValue(&WPD_OBJECT_NAME, &HSTRING::from(&leaf)).map_err(map)?;
        bag.SetGuidValue(&WPD_OBJECT_CONTENT_TYPE, &WPD_CONTENT_TYPE_FOLDER).map_err(map)?;
        let mut id = PWSTR::null();
        content.CreateObjectWithPropertiesOnly(&bag, &raw mut id).map_err(map)?;
        if !id.is_null() {
            CoTaskMemFree(Some(id.0 as *const _));
        }
        Ok(())
    }))
}

/// Removes one object. WPD removal is final — there is no device-side bin (documented gap).
pub fn remove(pnp: &str, storage: &str, path: &str) -> io::Result<()> {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let id = resolve(&content, &storage, &path)?;
        let ids = new_pvc()?;
        let mut pv = lpwstr(&id);
        if let Err(e) = ids.Add(&pv) {
            free_variant_string(&mut pv); // ownership only moves on success (probe-verified)
            return Err(map(e));
        }
        content.Delete(0, &ids, std::ptr::null_mut()).map_err(map)
    }))
}

/// Renames one object (the new name is a bare name, not a path).
pub fn rename(pnp: &str, storage: &str, path: &str, to: &str) -> io::Result<()> {
    let (pnp, storage, path, to) = (pnp.to_string(), storage.to_string(), path.to_string(), to.to_string());
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        let props = content.Properties().map_err(map)?;
        let id = resolve(&content, &storage, &path)?;
        let bag = new_values()?;
        bag.SetStringValue(&WPD_OBJECT_NAME, &HSTRING::from(&to)).map_err(map)?;
        props.SetValues(&HSTRING::from(&id), &bag).map(|_| ()).map_err(map)
    }))
}

/// Whether `path` names something on this storage.
pub fn exists(pnp: &str, storage: &str, path: &str) -> bool {
    let (pnp, storage, path) = (pnp.to_string(), storage.to_string(), path.to_string());
    call(move |ctx| ctx.with_device(&pnp, move |device| unsafe {
        let content = device.Content().map_err(map)?;
        resolve(&content, &storage, &path).map(|_| ())
    }))
    .is_ok()
}

/// Ejects through the device stack: query-and-remove the device subtree, which is what
/// stops MTP traffic cleanly (same path Explorer uses for "Safely Remove Hardware").
pub fn eject(pnp: &str) -> io::Result<()> {
    let pnp = pnp.to_string();
    call(move |ctx| {
        ctx.devices.remove(&pnp);
        unsafe {
            let mut node = 0u32;
            let found = CM_Locate_DevNodeW(&raw mut node, &HSTRING::from(&pnp), CM_LOCATE_DEVNODE_NORMAL);
            if found.0 != 0 {
                return Err(io::Error::other(format!("Windows could not find the device node ({found:?})")));
            }
            let mut veto = PNP_VETO_TYPE::default();
            let mut blocker = [0u16; 260];
            let ret = CM_Query_And_Remove_SubTreeW(node, Some(&raw mut veto), Some(&mut blocker), 0);
            if ret.0 != 0 {
                let who = String::from_utf16_lossy(&blocker).trim_matches('\0').to_string();
                let who = if who.is_empty() { String::new() } else { format!(", held by {who}") };
                return Err(io::Error::other(format!("Windows refused to eject the device ({ret:?}){who}")));
            }
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_answers_a_device_enumeration() {
        // Any Windows desktop answers this; proving the worker thread, its COM apartment and
        // the WPD manager all live is what this machine can verify without hardware.
        let devs = devices().expect("the WPD manager should answer enumeration on any Windows desktop");
        for d in devs {
            assert!(!d.pnp.is_empty(), "a device without a PnP path could not be addressed");
        }
    }

    #[test]
    fn filetimes_and_ole_dates_both_become_epoch_seconds() {
        use windows::Win32::Foundation::FILETIME;
        let stamp = 1_700_000_000i64;
        let ticks = stamp as u64 * 10_000_000 + FILETIME_UNIX_DIFF;
        let ft = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_FILETIME,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { filetime: FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 } },
                }),
            },
        };
        let ole = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_DATE,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { date: 25_569.0 },
                }),
            },
        };
        let empty = PROPVARIANT::default();
        unsafe {
            assert_eq!(mtime_variant(&ft), stamp);
            assert_eq!(mtime_variant(&ole), 0, "the OLE epoch is the Unix epoch");
            assert_eq!(mtime_variant(&empty), 0, "a hidden date stays 0");
        }
    }

    #[test]
    fn missing_objects_report_as_missing_and_denials_as_denied() {
        for code in [0x8003_0003u32, 0x8007_0002, 0x8007_0003] {
            let e = windows::core::Error::from_hresult(windows::core::HRESULT(code as i32));
            assert_eq!(map(e).kind(), io::ErrorKind::NotFound, "{code:#010x} must read as missing");
        }
        let e = windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0005u32 as i32));
        assert_eq!(map(e).kind(), io::ErrorKind::PermissionDenied, "a refusal must read as denied");
    }

    #[test]
    fn slash_paths_split_at_their_last_slash() {
        assert_eq!(split_path("a/b/c.jpg"), ("a/b".to_string(), "c.jpg".to_string()));
        assert_eq!(split_path("top.txt"), (String::new(), "top.txt".to_string()));
    }
}
