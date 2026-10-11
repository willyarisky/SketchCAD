//! The subset of the SketchUp C API the converter uses, loaded from the library at run time.
//!
//! Every API object is an opaque `{ void* ptr }` handle passed by value; functions return an
//! `SUResult` (0 = success). Matrices are column-major `double[16]`.

#![allow(non_snake_case)]

use std::ffi::{CString, c_char, c_int, c_void};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Ref(pub *mut c_void);

impl Ref {
    pub const NULL: Ref = Ref(std::ptr::null_mut());
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Pt {
    pub fn arr(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
    pub fn from(a: [f64; 3]) -> Pt {
        Pt { x: a[0], y: a[1], z: a[2] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Tr {
    pub values: [f64; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Col {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// `SUMaterialInput`: a material for a face, with optional texture coordinates (unused here).
#[repr(C)]
pub struct MaterialInput {
    pub num_uv_coords: usize,
    pub uv_coords: [[f64; 2]; 4],
    pub vertex_indices: [usize; 4],
    pub material: Ref,
}

pub const SU_ERROR_NONE: c_int = 0;
pub const SU_ERROR_MODEL_VERSION: c_int = 13;

type R = c_int;

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub struct Api {
            _lib: libloading::Library,
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            /// Load the library and resolve every function.
            ///
            /// # Safety
            /// `path` must be a SketchUp C API library: its exports are called with these signatures.
            pub unsafe fn load(path: &std::path::Path) -> Result<Api, String> {
                let lib = unsafe { libloading::Library::new(path) }.map_err(|e| format!("Could not load the SketchUp library {}: {e}", path.display()))?;
                $(
                    let $name = *unsafe { lib.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(concat!(stringify!($name), "\0").as_bytes()) }
                        .map_err(|_| format!("The SketchUp library has no {} (too old?)", stringify!($name)))?;
                )*
                Ok(Api { _lib: lib, $($name,)* })
            }
        }
    };
}

api! {
    SUInitialize: fn();
    SUTerminate: fn();
    SUModelCreate: fn(*mut Ref) -> R;
    SUModelCreateFromFile: fn(*mut Ref, *const c_char) -> R;
    SUModelRelease: fn(*mut Ref) -> R;
    SUModelSaveToFile: fn(Ref, *const c_char) -> R;
    SUModelGetVersion: fn(Ref, *mut c_int, *mut c_int, *mut c_int) -> R;
    SUModelGetEntities: fn(Ref, *mut Ref) -> R;
    SUModelGetUnits: fn(Ref, *mut c_int) -> R;
    SUModelGetCamera: fn(Ref, *mut Ref) -> R;
    SUModelAddLayers: fn(Ref, usize, *const Ref) -> R;
    SUModelAddMaterials: fn(Ref, usize, *const Ref) -> R;
    SUModelAddComponentDefinitions: fn(Ref, usize, *const Ref) -> R;
    SUModelGetNumScenes: fn(Ref, *mut usize) -> R;
    SUModelGetOptionsManager: fn(Ref, *mut Ref) -> R;
    SUOptionsManagerGetOptionsProviderByName: fn(Ref, *const c_char, *mut Ref) -> R;
    SUOptionsProviderSetValue: fn(Ref, *const c_char, Ref) -> R;
    SUTypedValueCreate: fn(*mut Ref) -> R;
    SUTypedValueSetInt32: fn(Ref, i32) -> R;
    SUTypedValueRelease: fn(*mut Ref) -> R;

    SUCameraGetOrientation: fn(Ref, *mut Pt, *mut Pt, *mut Pt) -> R;
    SUCameraSetOrientation: fn(Ref, *const Pt, *const Pt, *const Pt) -> R;
    SUCameraGetPerspective: fn(Ref, *mut bool) -> R;
    SUCameraSetPerspective: fn(Ref, bool) -> R;
    SUCameraGetPerspectiveFrustumFOV: fn(Ref, *mut f64) -> R;
    SUCameraSetPerspectiveFrustumFOV: fn(Ref, f64) -> R;

    SUEntitiesGetNumFaces: fn(Ref, *mut usize) -> R;
    SUEntitiesGetFaces: fn(Ref, usize, *mut Ref, *mut usize) -> R;
    SUEntitiesGetNumEdges: fn(Ref, bool, *mut usize) -> R;
    SUEntitiesGetEdges: fn(Ref, bool, usize, *mut Ref, *mut usize) -> R;
    SUEntitiesGetNumGroups: fn(Ref, *mut usize) -> R;
    SUEntitiesGetGroups: fn(Ref, usize, *mut Ref, *mut usize) -> R;
    SUEntitiesGetNumInstances: fn(Ref, *mut usize) -> R;
    SUEntitiesGetInstances: fn(Ref, usize, *mut Ref, *mut usize) -> R;
    SUEntitiesGetNumDimensions: fn(Ref, *mut usize) -> R;
    SUEntitiesGetNumImages: fn(Ref, *mut usize) -> R;
    SUEntitiesGetNumSectionPlanes: fn(Ref, *mut usize) -> R;
    SUEntitiesGetNumGuideLines: fn(Ref, *mut usize) -> R;
    SUEntitiesGetNumGuidePoints: fn(Ref, *mut usize) -> R;
    SUEntitiesFill: fn(Ref, Ref, bool) -> R;
    SUEntitiesAddGroup: fn(Ref, Ref) -> R;
    SUEntitiesAddInstance: fn(Ref, Ref, *mut Ref) -> R;

    SUFaceGetNormal: fn(Ref, *mut Pt) -> R;
    SUFaceGetOuterLoop: fn(Ref, *mut Ref) -> R;
    SUFaceGetNumInnerLoops: fn(Ref, *mut usize) -> R;
    SUFaceGetInnerLoops: fn(Ref, usize, *mut Ref, *mut usize) -> R;
    SUFaceGetFrontMaterial: fn(Ref, *mut Ref) -> R;
    SUFaceGetBackMaterial: fn(Ref, *mut Ref) -> R;
    SULoopGetNumVertices: fn(Ref, *mut usize) -> R;
    SULoopGetVertices: fn(Ref, usize, *mut Ref, *mut usize) -> R;
    SUVertexGetPosition: fn(Ref, *mut Pt) -> R;

    SUEdgeGetStartVertex: fn(Ref, *mut Ref) -> R;
    SUEdgeGetEndVertex: fn(Ref, *mut Ref) -> R;
    SUEdgeGetSoft: fn(Ref, *mut bool) -> R;
    SUEdgeGetSmooth: fn(Ref, *mut bool) -> R;
    SUEdgeGetCurve: fn(Ref, *mut Ref) -> R;
    SUEdgeToDrawingElement: fn(Ref) -> Ref;
    SUGroupToDrawingElement: fn(Ref) -> Ref;
    SUComponentInstanceToDrawingElement: fn(Ref) -> Ref;
    SUDrawingElementGetHidden: fn(Ref, *mut bool) -> R;
    SUDrawingElementSetHidden: fn(Ref, bool) -> R;
    SUDrawingElementGetLayer: fn(Ref, *mut Ref) -> R;
    SUDrawingElementSetLayer: fn(Ref, Ref) -> R;
    SUDrawingElementGetMaterial: fn(Ref, *mut Ref) -> R;
    SUDrawingElementSetMaterial: fn(Ref, Ref) -> R;

    SUGroupCreate: fn(*mut Ref) -> R;
    SUGroupGetEntities: fn(Ref, *mut Ref) -> R;
    SUGroupGetTransform: fn(Ref, *mut Tr) -> R;
    SUGroupSetTransform: fn(Ref, *const Tr) -> R;
    SUGroupGetName: fn(Ref, *mut Ref) -> R;
    SUGroupSetName: fn(Ref, *const c_char) -> R;

    SUComponentInstanceGetDefinition: fn(Ref, *mut Ref) -> R;
    SUComponentInstanceGetTransform: fn(Ref, *mut Tr) -> R;
    SUComponentInstanceSetTransform: fn(Ref, *const Tr) -> R;
    SUComponentInstanceGetName: fn(Ref, *mut Ref) -> R;
    SUComponentInstanceSetName: fn(Ref, *const c_char) -> R;
    SUComponentInstanceSetLocked: fn(Ref, bool) -> R;

    SUComponentDefinitionCreate: fn(*mut Ref) -> R;
    SUComponentDefinitionGetEntities: fn(Ref, *mut Ref) -> R;
    SUComponentDefinitionGetName: fn(Ref, *mut Ref) -> R;
    SUComponentDefinitionSetName: fn(Ref, *const c_char) -> R;
    SUComponentDefinitionGetDescription: fn(Ref, *mut Ref) -> R;
    SUComponentDefinitionSetDescription: fn(Ref, *const c_char) -> R;
    SUComponentDefinitionCreateInstance: fn(Ref, *mut Ref) -> R;

    SUMaterialCreate: fn(*mut Ref) -> R;
    SUMaterialSetName: fn(Ref, *const c_char) -> R;
    SUMaterialGetName: fn(Ref, *mut Ref) -> R;
    SUMaterialSetColor: fn(Ref, *const Col) -> R;
    SUMaterialGetColor: fn(Ref, *mut Col) -> R;
    SUMaterialGetTexture: fn(Ref, *mut Ref) -> R;
    SUMaterialGetUseOpacity: fn(Ref, *mut bool) -> R;
    SUMaterialGetOpacity: fn(Ref, *mut f64) -> R;
    SUMaterialSetOpacity: fn(Ref, f64) -> R;
    SUMaterialSetUseOpacity: fn(Ref, bool) -> R;

    SULayerCreate: fn(*mut Ref) -> R;
    SULayerSetName: fn(Ref, *const c_char) -> R;
    SULayerGetName: fn(Ref, *mut Ref) -> R;
    SULayerSetVisibility: fn(Ref, bool) -> R;
    SULayerGetVisibility: fn(Ref, *mut bool) -> R;

    SUStringCreate: fn(*mut Ref) -> R;
    SUStringGetUTF8Length: fn(Ref, *mut usize) -> R;
    SUStringGetUTF8: fn(Ref, usize, *mut c_char, *mut usize) -> R;
    SUStringRelease: fn(*mut Ref) -> R;

    SUGeometryInputCreate: fn(*mut Ref) -> R;
    SUGeometryInputRelease: fn(*mut Ref) -> R;
    SUGeometryInputSetVertices: fn(Ref, usize, *const Pt) -> R;
    SUGeometryInputAddEdge: fn(Ref, usize, usize, *mut usize) -> R;
    SUGeometryInputEdgeSetHidden: fn(Ref, usize, bool) -> R;
    SUGeometryInputEdgeSetSoft: fn(Ref, usize, bool) -> R;
    SUGeometryInputEdgeSetSmooth: fn(Ref, usize, bool) -> R;
    SUGeometryInputAddFace: fn(Ref, *mut Ref, *mut usize) -> R;
    SUGeometryInputFaceAddInnerLoop: fn(Ref, usize, *mut Ref) -> R;
    SUGeometryInputFaceSetFrontMaterial: fn(Ref, usize, *const MaterialInput) -> R;
    SUGeometryInputFaceSetBackMaterial: fn(Ref, usize, *const MaterialInput) -> R;
    SULoopInputCreate: fn(*mut Ref) -> R;
    SULoopInputAddVertexIndex: fn(Ref, usize) -> R;
    SULoopInputEdgeSetSoft: fn(Ref, usize, bool) -> R;
    SULoopInputEdgeSetSmooth: fn(Ref, usize, bool) -> R;
    SULoopInputEdgeSetHidden: fn(Ref, usize, bool) -> R;
    SULoopInputRelease: fn(*mut Ref) -> R;
}

/// Turn an `SUResult` into a Rust error naming the call.
pub fn ok(r: c_int, what: &str) -> Result<(), String> {
    if r == SU_ERROR_NONE { Ok(()) } else { Err(format!("{what} failed (SketchUp error {r})")) }
}

pub fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Api {
    /// Read an `SUStringRef` filled by `f`.
    pub fn string(&self, f: impl FnOnce(*mut Ref) -> c_int) -> String {
        unsafe {
            let mut s = Ref::NULL;
            if (self.SUStringCreate)(&mut s) != SU_ERROR_NONE {
                return String::new();
            }
            let mut out = String::new();
            if f(&mut s) == SU_ERROR_NONE {
                let mut len = 0usize;
                (self.SUStringGetUTF8Length)(s, &mut len);
                let mut buf = vec![0u8; len + 1];
                let mut copied = 0usize;
                if (self.SUStringGetUTF8)(s, buf.len(), buf.as_mut_ptr() as *mut c_char, &mut copied) == SU_ERROR_NONE {
                    buf.truncate(copied.min(len));
                    out = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
                }
            }
            (self.SUStringRelease)(&mut s);
            out
        }
    }

    /// Fetch a list via a `GetNum*` / `Get*` pair.
    pub fn list(&self, num: impl FnOnce(*mut usize) -> c_int, get: impl FnOnce(usize, *mut Ref, *mut usize) -> c_int) -> Vec<Ref> {
        let mut n = 0usize;
        if num(&mut n) != SU_ERROR_NONE || n == 0 {
            return Vec::new();
        }
        let mut v = vec![Ref::NULL; n];
        let mut got = 0usize;
        if get(n, v.as_mut_ptr(), &mut got) != SU_ERROR_NONE {
            return Vec::new();
        }
        v.truncate(got);
        v
    }
}
