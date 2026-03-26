//! BSN Asset Catalog: shared named asset definitions for BSN scenes.
//!
//! A `materials.bsn` (or any `.bsn` catalog file) defines reusable assets
//! that scenes reference by `#name` strings. The catalog uses standard BSN
//! grammar — each asset is an entry in a `Children [...]` block with a
//! `#Name` patch and a component struct patch.
//!
//! ## Example `materials.bsn`
//!
//! ```bsn
//! bevy_ecs::hierarchy::Children [
//!     #ground06
//!     bevy_pbr::pbr_material::StandardMaterial {
//!         base_color_texture: "ground06_basecolor.png",
//!         perceptual_roughness: 0.8,
//!     },
//!     #rock058
//!     bevy_pbr::pbr_material::StandardMaterial {
//!         base_color_texture: "rock058_basecolor.png",
//!     }
//! ]
//! ```
//!
//! Scenes reference these assets with `"#name"` strings in Handle fields:
//!
//! ```bsn
//! BrushFaceData {
//!     material: "#ground06",
//! }
//! ```

use bevy_asset::{AssetPath, AssetServer, UntypedHandle};
use bevy_ecs::{
    prelude::*,
    reflect::AppTypeRegistry,
};
use bevy_platform::collections::HashMap;
use bevy_reflect::{
    prelude::ReflectDefault,
    PartialReflect, ReflectMut, TypeRegistry,
};

use crate::dynamic_bsn::{BsnAst, BsnExpr, BsnNameStore, BsnPatch, BsnPatches};
use crate::dynamic_bsn_grammar::TopLevelPatchesParser;
use crate::dynamic_bsn_lexer::Lexer;

/// Resource holding named asset handles resolved from a BSN catalog file.
///
/// During scene loading, `Handle` fields containing `"#name"` strings are
/// resolved against this catalog.
#[derive(Resource, Default, Clone)]
pub struct BsnAssetCatalog {
    /// Maps asset names to their untyped handles.
    pub assets: HashMap<String, UntypedHandle>,
}

impl BsnAssetCatalog {
    /// Look up a named asset handle.
    pub fn get(&self, name: &str) -> Option<&UntypedHandle> {
        self.assets.get(name)
    }

    /// Insert a named asset handle.
    pub fn insert(&mut self, name: String, handle: UntypedHandle) {
        self.assets.insert(name, handle);
    }

    /// Check if a string is a catalog reference (starts with `#`).
    pub fn is_catalog_ref(s: &str) -> bool {
        s.starts_with('#')
    }

    /// Extract the name from a catalog reference string (strips the `#` prefix).
    pub fn ref_name(s: &str) -> &str {
        s.strip_prefix('#').unwrap_or(s)
    }
}

/// Parse a BSN catalog file and populate a [`BsnAssetCatalog`].
///
/// The catalog file uses standard BSN grammar. Each root entity is expected
/// to have a `#Name` patch and one or more struct patches representing the
/// asset data. The struct patches are applied via reflection to create the
/// assets, which are then inserted into the Bevy `Assets<T>` stores.
///
/// Currently supports `StandardMaterial` and any type registered with
/// `ReflectDefault` + `ReflectComponent`.
pub fn load_bsn_catalog(
    world: &mut World,
    catalog_text: &str,
) -> Result<BsnAssetCatalog, String> {
    // Parse using the BSN grammar
    let mut parse_world = World::new();
    parse_world.init_resource::<BsnNameStore>();
    let ast = core::cell::RefCell::new(BsnAst(parse_world));

    let lexer = Lexer::new(catalog_text);
    let patches_id = TopLevelPatchesParser::new()
        .parse(&ast, lexer)
        .map_err(|e| format!("BSN catalog parse error: {e:?}"))?;

    let bsn_ast = ast.into_inner();

    // Unwrap top-level Children wrapper to get individual asset entries
    let root_entries = unwrap_catalog_roots(&bsn_ast, patches_id)?;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>().cloned();

    let mut catalog = BsnAssetCatalog::default();

    for entry_id in root_entries {
        let Some(patches) = bsn_ast.0.get::<BsnPatches>(entry_id) else {
            continue;
        };

        let mut name: Option<String> = None;
        let mut asset_handle: Option<UntypedHandle> = None;

        for &patch_id in &patches.0 {
            let Some(patch) = bsn_ast.0.get::<BsnPatch>(patch_id) else {
                continue;
            };
            match patch {
                BsnPatch::Name(n, _) => {
                    name = Some(n.clone());
                }
                BsnPatch::Struct(bsn_struct) => {
                    // Try to create the asset from the struct data
                    let type_path = bsn_struct.0.as_path();
                    if let Some(handle) = create_asset_from_struct(
                        world,
                        &type_path,
                        &bsn_struct.1,
                        &bsn_ast,
                        &reg,
                        asset_server.as_ref(),
                    ) {
                        asset_handle = Some(handle);
                    }
                }
                BsnPatch::Var(var) => {
                    // Bare type (all defaults) — create default asset
                    let type_path = var.0.as_path();
                    if let Some(handle) =
                        create_default_asset(world, &type_path, &reg)
                    {
                        asset_handle = Some(handle);
                    }
                }
                _ => {}
            }
        }

        if let (Some(name), Some(handle)) = (name, asset_handle) {
            catalog.insert(name, handle);
        }
    }

    Ok(catalog)
}

/// Unwrap a top-level Children wrapper into individual root entries.
fn unwrap_catalog_roots(
    ast: &BsnAst,
    patches_id: Entity,
) -> Result<Vec<Entity>, String> {
    let Some(patches) = ast.0.get::<BsnPatches>(patches_id) else {
        return Err("No top-level patches found".to_string());
    };

    // Check if this is a Children wrapper (single Relation patch)
    if patches.0.len() == 1 {
        if let Some(BsnPatch::Relation(relation)) = ast.0.get::<BsnPatch>(patches.0[0]) {
            // It's a Children wrapper — return the child entities
            return Ok(relation.1.clone());
        }
    }

    // Not wrapped — treat the top-level as a single entry
    Ok(vec![patches_id])
}

/// Create an asset from BSN struct fields via reflection, returning an untyped handle.
fn create_asset_from_struct(
    world: &mut World,
    type_path: &str,
    fields: &[crate::dynamic_bsn::BsnField],
    ast: &BsnAst,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<UntypedHandle> {
    let registration = registry.get_with_type_path(type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;

    let mut value = reflect_default.default();

    // Apply struct fields
    if let ReflectMut::Struct(s) = value.reflect_mut() {
        let struct_info = registration.type_info().as_struct().ok()?;
        for field in fields {
            let Some(field_info) = struct_info.field(&field.0) else {
                continue;
            };

            // Get the expression value
            let Some(expr) = ast.0.get::<BsnExpr>(field.1) else {
                continue;
            };

            apply_bsn_expr_to_field(s, &field.0, expr, field_info.ty().id(), registry, asset_server, ast)?;
        }
    }

    // Insert as an asset — we need to use the reflected asset insertion
    // For now, try to insert via the ReflectComponent pathway (works for Asset types
    // that are also Components, like StandardMaterial). For pure assets, we'd need
    // ReflectAsset type data.
    //
    // Generic approach: use world.resource_scope with Assets<T> if we know T.
    // For now, return the reflected value and let the caller handle insertion.
    insert_reflected_asset(world, registration.type_id(), value.as_partial_reflect())
}

/// Create a default asset of the given type.
fn create_default_asset(
    world: &mut World,
    type_path: &str,
    registry: &TypeRegistry,
) -> Option<UntypedHandle> {
    let registration = registry.get_with_type_path(type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let value = reflect_default.default();
    insert_reflected_asset(world, registration.type_id(), value.as_partial_reflect())
}

/// Insert a reflected value as an asset and return its untyped handle.
fn insert_reflected_asset(
    world: &mut World,
    _type_id: core::any::TypeId,
    value: &dyn PartialReflect,
) -> Option<UntypedHandle> {
    // Use ReflectAsset if available
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    if let Some(reflect_asset) = reg.get(_type_id)?.data::<bevy_asset::ReflectAsset>() {
        let handle = reflect_asset.add(world, value);
        return Some(handle);
    }

    None
}

/// Apply a BSN expression to a struct field via reflection.
fn apply_bsn_expr_to_field(
    target_struct: &mut dyn bevy_reflect::structs::Struct,
    field_name: &str,
    expr: &BsnExpr,
    expected_type_id: core::any::TypeId,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    ast: &BsnAst,
) -> Option<()> {
    let target = target_struct.field_mut(field_name)?;

    match expr {
        BsnExpr::FloatLit(f) => {
            if expected_type_id == core::any::TypeId::of::<f32>() {
                target.apply(&(*f as f32));
            } else if expected_type_id == core::any::TypeId::of::<f64>() {
                target.apply(f);
            }
        }
        BsnExpr::IntLit(i) => {
            // Try common integer types
            macro_rules! try_apply_int {
                ($($t:ty),*) => {
                    $(if expected_type_id == core::any::TypeId::of::<$t>() {
                        target.apply(&(*i as $t));
                        return Some(());
                    })*
                };
            }
            try_apply_int!(i8, u8, i16, u16, i32, u32, i64, u64, usize, isize);
        }
        BsnExpr::BoolLit(b) => {
            target.apply(b);
        }
        BsnExpr::StringLit(s) => {
            // Check if target is a Handle<T> — load as asset path
            if let Some(asset_server) = asset_server {
                if let Some(concrete) = target.try_as_reflect() {
                    let type_id = concrete.reflect_type_info().type_id();
                    if registry.get_type_data::<bevy_asset::ReflectHandle>(type_id).is_some() {
                        // Load the asset via AssetServer
                        let handle = asset_server.load_untyped(AssetPath::parse(s).into_owned());
                        target.apply(handle.as_partial_reflect());
                        return Some(());
                    }
                }
            }
            // Regular string
            if expected_type_id == core::any::TypeId::of::<String>() {
                target.apply(&s.clone());
            }
        }
        BsnExpr::Struct(bsn_struct) => {
            // Nested struct — recurse
            let type_path = bsn_struct.0.as_path();
            if let Some(registration) = registry.get_with_type_path(&type_path) {
                if let Some(struct_info) = registration.type_info().as_struct().ok() {
                    if let ReflectMut::Struct(s) = target.reflect_mut() {
                        for field in &bsn_struct.1 {
                            if let Some(fi) = struct_info.field(&field.0) {
                                if let Some(expr) = ast.0.get::<BsnExpr>(field.1) {
                                    apply_bsn_expr_to_field(
                                        s, &field.0, expr, fi.ty().id(),
                                        registry, asset_server, ast,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Some(())
}
