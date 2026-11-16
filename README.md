# IFC-Lite for .NET — AB4D fork

This is a fork of [IFC-Lite](https://github.com/LTplus-AG/ifc-lite), maintained
for direct access to IFC mesh geometry from .NET applications. The original
project provides the Rust IFC parser and geometry engine; this fork adds a
C-compatible streaming interface that .NET can call through P/Invoke or
unmanaged function pointers.

The native library can be used by the
[Ab4d.SharpEngine.IfcLight NuGet package](https://www.nuget.org/packages/Ab4d.SharpEngine.IfcLight/)
to import IFC geometry into AB4D SharpEngine.

## See the importer in action

Run [Ab4d.SharpEngine.Samples](https://github.com/ab4d/Ab4d.SharpEngine.Samples)
and open "**3D model importers / IFC and BIM importer**".

## Fork changes and branches

The `ab4d/net-interop-v12.2.0` branch is based on upstream release `v12.2.0`.
Its .NET integration is implemented in
[`rust/ffi/src/geometry_interop.rs`](rust/ffi/src/geometry_interop.rs), connected to the existing
FFI crate through [`rust/ffi/src/lib.rs`](rust/ffi/src/lib.rs).

The changes add:

- **Streaming mesh access:** `geometry_processor_new`,
  `geometry_processor_process_adaptive`, and `geometry_processor_free` provide
  a processor handle and callbacks for batches of geometry. Mesh positions,
  normals, and triangle indices are exposed as native arrays, avoiding JSON
  serialization of geometry.
- **Element information:** each mesh descriptor carries its EXPRESS ID, IFC
  type, GlobalId, name, presentation layer, properties JSON, and RGBA color.
- **Coordinate information:** mesh descriptors include the three `f64` origin
  components and six `f32` local bounds components. A model metadata callback
  supplies coordinate-space, origin-shift, unit-scale, and available site and
  building transforms.
- **Processing options:** callers can select opening filtering, initial and
  subsequent batch sizes, first-batch prioritization, property and layer
  inclusion, and tessellation quality.
- **Current pipeline integration:** processing uses upstream's
  `ProcessingRequest` API and shared geometry worker pool, with Rust panic
  handling around the operation.
- **Engine version access:** `ifc_lite_version()` returns the Rust workspace
  version as a borrowed, null-terminated UTF-8 string.

Upstream's existing JSON-based `ifc_lite_parse*` and `ifc_lite_free` exports
remain available. This fork's additions adapt the upstream geometry pipeline
for native consumers; geometry generation continues to use the Rust engine.

## Build the native DLL

### Build on Windows
On Windows, install Rust and the Visual Studio C++ build tools, including the
Windows SDK. From the repository root, build the x64 DLL with:

```powershell
cargo +nightly build --profile server-release -p ifc-lite-ffi --target x86_64-pc-windows-msvc
```

Output:

```text
target/x86_64-pc-windows-msvc/server-release/ifc_lite_ffi.dll
```

The repository pins its upstream Rust toolchain in `rust-toolchain.toml`.
Omit `+nightly` to use that pinned toolchain instead of your installed nightly.
The native FFI build does not require the browser viewer or a WASM build.

Use the **server-release** profile: it enables panic unwinding so the FFI can
return an error rather than abort the .NET host on a recoverable Rust panic.
The ordinary `--release` build uses aborting panics and is refused by this crate.

The DLL architecture must match the .NET process. For x86, install the
`i686-pc-windows-msvc` Rust target and the corresponding Visual Studio tools,
then build with `--target i686-pc-windows-msvc` into its separate target directory.

### Build on macOS

Install Rust and use the following command:
```bash
cargo +nightly build --profile server-release -p ifc-lite-ffi --target aarch64-apple-darwin
```

Output:
```text
target/aarch64-apple-darwin/server-release/libifc_lite_ffi.dylib
```

You also need to sign the library. If you do not have the Apple Developer Id, you can use the ad-hoc certificate:
```
codesign --force --deep --sign - libifc_lite_ffi.dylib
```

If you used ad-hoc certificate, then you also need make the library trusted with the following:
```
xattr -d com.apple.quarantine libifc_lite_ffi.dylib
```

This last step is also needed when the library is copied to another computer.
If this is not done, then "Apple could not verify “libifc_lite_ffi.dylib” is free of malware that may harm your Mac or compromise your privacy." message box
is displayed and the NativeLibrary.TryLoad method in .Net fails.

### Build on Linux

Install Rust and use the following command:
```bash
cargo +nightly build --profile server-release -p ifc-lite-ffi --target x86_64-unknown-linux-gnu
```

Output:
```text
target/x86_64-unknown-linux-gnu/server-release/libifc_lite_ffi.so
```

## Native interop contract

Match the Rust `#[repr(C)]` structures and C calling convention in your managed
declarations. Rust `usize` corresponds to .NET `nuint` or `UIntPtr`, and native
pointers correspond to `IntPtr` or unsafe pointers. The mesh descriptor layout
includes the additional origin and bounds fields; keep managed declarations
in sync with the DLL you distribute.

Mesh arrays and UTF-8 strings are borrowed and valid only during each callback.
Copy the data before returning. Keep input buffers and callback delegates alive
until processing finishes. Callbacks run on a worker thread; catch managed
exceptions inside them. Free processor handles with `geometry_processor_free`.

The optional model metadata callback runs once **after** the mesh batches.
Absent local bounds are represented by six NaN values. Passing a null options
pointer uses the defaults, including properties and presentation layers;
passing a zero-initialized options struct disables those two features.

The branch's upstream release version is `12.2.0`, while the Rust workspace
version is `28.2.0`. Upstream tracks Rust-only breaking changes through
[`rust-major-offset.json`](rust-major-offset.json), so these versions can differ.
`ifc_lite_version()` reports the Rust engine version; copy its UTF-8 string
before unloading the DLL and do not free the returned pointer.

## Upstream documentation and attribution

For the original project's capabilities, architecture, browser viewer, CLI,
and development workflow, see:

- [Original IFC-Lite repository](https://github.com/LTplus-AG/ifc-lite)
- [IFC-Lite documentation](https://www.ifclite.dev/)
- [Documentation in this checkout](docs/)

The IFC parser and geometry engine are the work of the IFC-Lite contributors.
The .NET mesh interop additions are by Andrej Benedik / AB4D.

## License

This fork retains the upstream [Mozilla Public License 2.0](LICENSE).
Preserve applicable copyright and license notices and comply with the MPL-2.0
requirements when distributing covered source files and modifications.
