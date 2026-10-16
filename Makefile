# failBrauwser build entry points. `make` builds everything into target/.
CWB_ROOT ?= $(CURDIR)/vendor/compressionworkbench
HELPER_OUT := target/helper
# Extra `dotnet publish` options, e.g. -p:CppCompilerAndLinker=gcc where clang is missing.
HELPER_FLAGS ?=

.PHONY: all app helper test clean

all: app helper

app:
	cargo build --release

helper:
	dotnet publish helper/FbArchive/FbArchive.csproj -c Release -r linux-x64 \
		"-p:CwbRoot=$(CWB_ROOT)" $(HELPER_FLAGS) -o $(HELPER_OUT) --nologo -v quiet
	rm -f $(HELPER_OUT)/*.pdb $(HELPER_OUT)/*.xml $(HELPER_OUT)/*.dbg

test: helper
	dotnet test helper/FbArchive.Tests/FbArchive.Tests.csproj "-p:CwbRoot=$(CWB_ROOT)" --nologo -v quiet
	cargo test

clean:
	cargo clean
	rm -rf helper/FbArchive/bin helper/FbArchive/obj
