# build-script-sdk

A package for a native library, zlib, whose build script decides where the library comes from:
the system's copy, or the sources the package carries.

- `packages/zlib` - the package.  Its `library` target `z` lists zlib 1.3.1's sources and claims the
  native library `z` with `links`.  Its build script, `build.c`, reads the package's features:
  - with `vendored`, it reports nothing, and Cabin compiles the listed sources;
  - without it, it reports the target `external` and names the system's zlib in its place: the SDK
    in the directory `CABIN_SDK_ZLIB_DIR` names when you set that variable (`include/zlib.h` and a
    library under `lib/`), and otherwise the module `zlib` that `pkg-config` knows.
- `packages/packer` - a C `library` whose public header includes `zlib.h`, so it declares zlib a
  public dependency.
- `packages/app` - a C `executable` that uses zlib only through `packer.h`.  Its build script packs
  `assets/banner.txt` with zlib into a header, and the program unpacks it.
- `packages/appxx` - a C++ `executable` that uses zlib directly.

The script never switches by itself: a system zlib that is missing or older than 1.2.11 fails the
build.  The version it asks of the system (1.2.11 or newer) is not the package's own version
(1.3.1, the sources it carries).  For an SDK directory it reads the version from `zlib.h`, and
does not check that the library file beside it is that version.

Dependents name the target `zlib:z` either way, and a program links one zlib: when any package
in a build asks for `vendored`, every dependent gets the vendored library.  `app` and `appxx`
pass the choice on with a feature of their own, `vendored = ["zlib/vendored"]`.

`app`'s build script is a program of the machine the build runs on, and the zlib it links is the
package's build dependency, which has features of its own: it is always `vendored`, whatever zlib
the program links.  The program's zlib is never the build script's.

See [`docs/build-scripts.md`](../../docs/build-scripts.md#supplying-a-library) for what a build
script may report about a library it supplies.

## Build and run

With zlib built from the package's sources:

```sh
cd examples/build-script-sdk
cabin run --features vendored
```

Expected output:

```
Hello from a banner packed with zlib
packed by zlib 1.3.1, unpacked by zlib 1.3.1 (header 1.3.1)
```

With the system's zlib, where `pkg-config` knows the module `zlib` (not under MSVC, whose
libraries `pkg-config` does not describe):

```sh
cabin run
```

With an SDK in a directory of its own, on any platform.  Name it by its absolute path:

```sh
CABIN_SDK_ZLIB_DIR=/opt/zlib cabin run
```

The last line then names the system's version after `unpacked by`.  The C++ program is
`cabin run -p appxx`, with the same choices.
