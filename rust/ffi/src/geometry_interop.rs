// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{ensure_panic_logging, run_in_pool};
use ifc_lite_processing::{process, Batching, Features, OnBatch, ProcessingRequest};
use std::slice;

// Added by Andrej Benedik (AB4D) to add support to read native mesh arrays from c#

// ── Binary streaming FFI (geometry_processor_*) ──────────────────────────────
//
// Zero-JSON P/Invoke surface for C# callers that want raw float/uint arrays.
// Three exports mirror the NativeMethods.cs declarations:
//   geometry_processor_new  → allocates an opaque handle (trivial, future-proof)
//   geometry_processor_free → frees it
//   geometry_processor_process_adaptive → runs the pipeline, fires callback per batch

/// Opaque processor handle. Currently a unit struct; may grow config fields later.
pub struct GeometryProcessor;

/// C-layout mesh descriptor. All pointers are valid ONLY for the duration of
/// the MeshBatchCallback invocation — copy the data before returning.
///
/// All string pointers (`ifc_type_ptr`, `name_ptr`, `layer_ptr`,
/// `properties_json_ptr`, `material_name_ptr`) point to null-terminated UTF-8 bytes and are
/// NEVER null — absent/empty fields use an empty C-string (`"\0"`).
/// `properties_json_ptr` points to a JSON object string, e.g.
/// `{"FireRating":"REI 90","IsExternal":"true"}\0`, or `{}\0` when
/// the mesh has no properties.

#[repr(C)]
pub struct NativeMesh {
    pub express_id: u32,
    /// Source geometry item EXPRESS id, or 0 when absent.
    pub geometry_item_id: u32,
    pub global_id_ptr: *const u8,
    /// Null-terminated UTF-8 element name, or `"\0"` when absent.
    pub name_ptr: *const u8,
    /// Null-terminated UTF-8 IFC type name, e.g. `"IfcWall\0"`.
    pub ifc_type_ptr: *const u8,
    /// Null-terminated UTF-8 presentation layer, or `"\0"` when absent.
    pub layer_ptr: *const u8,
    /// Null-terminated UTF-8 material name, or `"\0"` when absent.
    pub material_name_ptr: *const u8,
    /// Null-terminated UTF-8 JSON object of flat string properties,
    /// e.g. `{"IsExternal":"true"}\0`. Always `"{}\0"` when none.
    pub properties_json_ptr: *const u8,
    pub color_r: f32,
    pub color_g: f32,
    pub color_b: f32,
    pub color_a: f32,
    /// Per-mesh origin, copied from MeshData::origin without narrowing.
    pub origin_x: f64,
    pub origin_y: f64,
    pub origin_z: f64,
    /// Object-space AABB; all six values are NaN when local_bounds is absent.
    pub local_bounds_min_x: f32,
    pub local_bounds_min_y: f32,
    pub local_bounds_min_z: f32,
    pub local_bounds_max_x: f32,
    pub local_bounds_max_y: f32,
    pub local_bounds_max_z: f32,
    pub vertex_count: u32,
    pub index_count: u32,
    pub position_ptr: *const f32,
    pub normal_ptr: *const f32,
    pub index_ptr: *const u32,
}

/// Tessellation quality levels, mirroring `ifc_lite_geometry::TessellationQuality`.
/// Pass `TessellationQuality_Default` (0) to use the engine default (`Medium`).
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum FfiTessellationQuality {
    Default = 0,
    Lowest = 1,
    Low = 2,
    Medium = 3,
    High = 4,
    Highest = 5,
}

/// Options for `geometry_processor_process_adaptive`.
/// Zero-initialise and set only the fields you want to override; all fields
/// have well-documented defaults that match the library's own defaults.
#[repr(C)]
pub struct ProcessAdaptiveOptions {
    /// Opening filter: 0 = Default (cuts voids), 1 = IgnoreAll, 2 = IgnoreOpaque.
    pub opening_filter_mode: i32,

    /// Number of elements in the very first emitted batch.
    /// 0 or negative → use default (50). Smaller = lower first-batch latency.
    pub initial_batch_size: i32,

    /// Number of elements in subsequent batches after the first.
    /// 0 or negative → use default (50). Larger = higher throughput.
    pub throughput_batch_size: i32,

    /// When non-zero, sort high-value element types (walls, slabs, columns …)
    /// into the first batch so the most visible geometry arrives earliest.
    pub fast_first_batch: i32,

    /// When zero, skip IFC property-set parsing (faster for pure geometry use).
    /// Non-zero (default behaviour) includes properties.
    pub include_properties: i32,

    /// When zero, skip presentation-layer name resolution.
    /// Non-zero (default) includes layer names.
    pub include_presentation_layers: i32,

    /// Tessellation quality for curved geometry.
    /// 0 = engine default (Medium); see `FfiTessellationQuality`.
    pub tessellation_quality: i32,

    /// Reserved for future options without breaking the ABI. Always zero-init.
    pub _reserved: [i32; 8],
}

impl ProcessAdaptiveOptions {
    /// Produce a `ProcessingRequest` from the caller-supplied values, applying
    /// defaults for any zero / out-of-range fields.
    fn to_request(&self) -> ProcessingRequest {
        use ifc_lite_processing::TessellationQuality; // re-exported from ifc_lite_processing

        let initial = if self.initial_batch_size > 0 {
            self.initial_batch_size as usize
        } else {
            50
        };
        let throughput = if self.throughput_batch_size > 0 {
            self.throughput_batch_size as usize
        } else {
            50
        };

        let tessellation_quality = match self.tessellation_quality {
            1 => TessellationQuality::Lowest,
            2 => TessellationQuality::Low,
            3 => TessellationQuality::Medium,
            4 => TessellationQuality::High,
            5 => TessellationQuality::Highest,
            _ => TessellationQuality::default(),
        };

        ProcessingRequest::default()
            .opening_filter(self.to_opening_filter())
            .quality(tessellation_quality)
            .batching(Batching::new(initial, throughput).fast_first(self.fast_first_batch != 0))
            .features(
                Features::default()
                    .properties(self.include_properties != 0)
                    .presentation_layers(self.include_presentation_layers != 0),
            )
            .retain_meshes(false) // data lives only during the callback
    }

    fn to_opening_filter(&self) -> ifc_lite_processing::OpeningFilterMode {
        match self.opening_filter_mode {
            1 => ifc_lite_processing::OpeningFilterMode::IgnoreAll,
            2 => ifc_lite_processing::OpeningFilterMode::IgnoreOpaque,
            _ => ifc_lite_processing::OpeningFilterMode::Default,
        }
    }
}

/// Batch event passed to the callback.
#[repr(C)]
pub struct MeshBatchEvent {
    pub meshes_array_ptr: *const NativeMesh,
    pub mesh_count: u32,
}

/// Return the build's workspace version as a null-terminated UTF-8 string.
///
/// The pointer is non-null and valid until the library is unloaded. The caller
/// must not modify or free it. Copy the string before unloading the library.
#[no_mangle]
pub extern "C" fn ifc_lite_version() -> *const std::ffi::c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Callback type fired once per geometry batch. Pointers inside the event are
/// valid only for the duration of each call.
pub type MeshBatchCallback = unsafe extern "C" fn(event: MeshBatchEvent);

/// Called once after all mesh batches have been emitted. Delivers model-level coordinate metadata as UTF-8 JSON.
/// The pointer is valid only for the duration of this call.
///
/// JSON shape (optional fields omitted when not present in the IFC file):
/// ```json
/// {
///   "mesh_coordinate_space": "site_local",
///   "origin_shift":          [x, y, z],
///   "length_unit_scale":     0.001,
///   "site_transform":        [16 f64 values, column-major],
///   "building_transform":    [16 f64 values, column-major]
/// }
/// ```
pub type ModelMetadataCallback = unsafe extern "C" fn(json_ptr: *const u8, json_len: usize);

/// Allocate a new geometry processor.
/// Free with `geometry_processor_free`.
///
/// # Safety
/// Free the returned handle exactly once with `geometry_processor_free`.
#[no_mangle]
pub unsafe extern "C" fn geometry_processor_new() -> *mut GeometryProcessor {
    Box::into_raw(Box::new(GeometryProcessor))
}

/// Free a processor allocated by `geometry_processor_new`.
///
/// # Safety
/// `processor` must be null or a live handle returned by `geometry_processor_new`.
/// It must not be used or freed again after this call.
#[no_mangle]
pub unsafe extern "C" fn geometry_processor_free(processor: *mut GeometryProcessor) {
    if !processor.is_null() {
        drop(Box::from_raw(processor));
    }
}

fn opt_to_cstr(s: Option<&str>) -> std::ffi::CString {
    std::ffi::CString::new(s.unwrap_or("")).unwrap_or_default()
}

fn properties_to_cstr(
    props: Option<&std::collections::BTreeMap<String, String>>,
) -> std::ffi::CString {
    let json = match props {
        None => "{}".to_string(),
        Some(map) if map.is_empty() => "{}".to_string(),
        Some(map) => serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string()),
    };
    std::ffi::CString::new(json).unwrap_or_default()
}

/// Process IFC bytes, stream geometry batches to `mesh_callback`, then call
/// `metadata_callback` once with model coordinate metadata.
///
/// # Returns
/// - `0` success, `1` null pointer, `3` geometry panic or unavailable worker pool,
///   `4` metadata serialization failure
///
/// # Safety
/// `processor` must be a live handle. `file_buffer` must point to `buffer_size`
/// readable bytes in one allocation, with size at most `isize::MAX`.
/// `options` must be null or point to a valid, aligned options value.
/// Inputs must remain valid throughout the call. Callbacks must not unwind or
/// let managed exceptions escape, and must copy borrowed data before returning.
/// Callbacks run synchronously on a worker thread; keep managed delegates alive
/// until this function returns.
#[no_mangle]
pub unsafe extern "C" fn geometry_processor_process_adaptive(
    processor: *mut GeometryProcessor,
    file_buffer: *const u8,
    buffer_size: usize,
    options: *const ProcessAdaptiveOptions,
    mesh_callback: MeshBatchCallback,
    metadata_callback: Option<ModelMetadataCallback>,
) -> i32 {
    ensure_panic_logging();
    if processor.is_null() || file_buffer.is_null() {
        return 1;
    }
    let content = slice::from_raw_parts(file_buffer, buffer_size);

    let defaults = ProcessAdaptiveOptions {
        opening_filter_mode: 0,
        initial_batch_size: 0,
        throughput_batch_size: 0,
        fast_first_batch: 0,
        include_properties: 1,
        include_presentation_layers: 1,
        tessellation_quality: 0,
        _reserved: [0; 8],
    };
    let opts: &ProcessAdaptiveOptions = if options.is_null() {
        &defaults
    } else {
        &*options
    };
    let request = opts.to_request();

    run_in_pool("<P/Invoke memory buffer>", || {
        let mut sink = OnBatch::new(
            |meshes: &[ifc_lite_processing::MeshData], _processed, _total| {
                // Build CStrings for every mesh in this batch. They live until
                // `cstrings` drops at the end of this closure invocation.
                let cstrings: Vec<(
                    std::ffi::CString, // ifc_type
                    std::ffi::CString, // global_id
                    std::ffi::CString, // name
                    std::ffi::CString, // layer
                    std::ffi::CString, // properties_json
                    std::ffi::CString, // material_name
                )> = meshes
                    .iter()
                    .filter(|m| !m.positions.is_empty() && !m.indices.is_empty())
                    .map(|m| {
                        (
                            opt_to_cstr(Some(m.ifc_type.as_str())),
                            opt_to_cstr(m.global_id.as_deref()),
                            opt_to_cstr(m.name.as_deref()),
                            opt_to_cstr(m.presentation_layer.as_deref()),
                            properties_to_cstr(m.properties.as_ref()),
                            opt_to_cstr(m.material_name.as_deref()),
                        )
                    })
                    .collect();

                let native: Vec<NativeMesh> = meshes
                    .iter()
                    .filter(|m| !m.positions.is_empty() && !m.indices.is_empty())
                    .zip(cstrings.iter())
                    .map(
                        |(m, (ifc_cs, global_id_cs, name_cs, layer_cs, props_cs, material_cs))| NativeMesh {
                            position_ptr: m.positions.as_ptr(),
                            normal_ptr: m.normals.as_ptr(),
                            index_ptr: m.indices.as_ptr(),
                            ifc_type_ptr: ifc_cs.as_ptr() as *const u8,
                            global_id_ptr: global_id_cs.as_ptr() as *const u8,
                            name_ptr: name_cs.as_ptr() as *const u8,
                            layer_ptr: layer_cs.as_ptr() as *const u8,
                            properties_json_ptr: props_cs.as_ptr() as *const u8,
                            express_id: m.express_id,
                            vertex_count: (m.positions.len() / 3) as u32,
                            index_count: m.indices.len() as u32,
                            color_r: m.color[0],
                            color_g: m.color[1],
                            color_b: m.color[2],
                            color_a: m.color[3],
                            origin_x: m.origin[0],
                            origin_y: m.origin[1],
                            origin_z: m.origin[2],
                            local_bounds_min_x: m.local_bounds.map_or(f32::NAN, |b| b[0]),
                            local_bounds_min_y: m.local_bounds.map_or(f32::NAN, |b| b[1]),
                            local_bounds_min_z: m.local_bounds.map_or(f32::NAN, |b| b[2]),
                            local_bounds_max_x: m.local_bounds.map_or(f32::NAN, |b| b[3]),
                            local_bounds_max_y: m.local_bounds.map_or(f32::NAN, |b| b[4]),
                            local_bounds_max_z: m.local_bounds.map_or(f32::NAN, |b| b[5]),
                            material_name_ptr: material_cs.as_ptr() as *const u8,
                            geometry_item_id: m.geometry_item_id.unwrap_or(0),
                        },
                    )
                    .collect();

                if !native.is_empty() {
                    mesh_callback(MeshBatchEvent {
                        mesh_count: native.len() as u32,
                        meshes_array_ptr: native.as_ptr(),
                    });
                }
                // `cstrings` and `native` drop here — all pointers invalidated
            },
        );
        let processing_result = process(content, &request, &mut sink);

        // Fire metadata_callback once, after all batches, with the coordinate data
        // that is only available after the preprocess + geometry phases complete.
        if let Some(cb) = metadata_callback {
            let meta = &processing_result.metadata;
            let mut doc = serde_json::json!({
                "mesh_coordinate_space": processing_result.mesh_coordinate_space,
                "origin_shift":          meta.coordinate_info.origin_shift,
                "length_unit_scale":     meta.length_unit_scale,
            });
            if let Some(ref st) = processing_result.site_transform {
                doc["site_transform"] = serde_json::json!(st);
            }
            if let Some(ref bt) = processing_result.building_transform {
                doc["building_transform"] = serde_json::json!(bt);
            }
            let bytes = match serde_json::to_vec(&doc) {
                Ok(b) => b,
                Err(_) => return Err(4),
            };
            cb(bytes.as_ptr(), bytes.len());
            // `bytes` drops here — pointer invalidated
        }

        Ok(())
    })
    .map_or_else(|code| code, |()| 0)
}
