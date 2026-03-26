//! BSN asset loading: parse a `.bsn` file containing named asset definitions
//! and insert them into the Bevy asset stores via reflection.
//!
//! A `materials.bsn` file uses standard BSN grammar. Each entry has a `#Name`
//! and a struct patch with the asset data.

use bevy_asset::{AssetPath, AssetServer};
use bevy_ecs::{prelude::*, reflect::AppTypeRegistry};
use bevy_reflect::{prelude::ReflectDefault, PartialReflect, ReflectMut, TypeRegistry};

use crate::dynamic_bsn::{BsnAst, BsnExpr, BsnNameStore, BsnPatch, BsnPatches};
use crate::dynamic_bsn_grammar::TopLevelPatchesParser;
use crate::dynamic_bsn_lexer::Lexer;

/// Parse a BSN file containing named asset definitions and insert them into
/// the corresponding `Assets<T>` stores via reflection.
///
/// Returns a list of `(name, UntypedHandle)` pairs for the created assets.
pub fn load_bsn_assets(
    world: &mut World,
    bsn_text: &str,
) -> Result<Vec<(String, bevy_asset::UntypedHandle)>, String> {
    let mut parse_world = World::new();
    parse_world.init_resource::<BsnNameStore>();
    let ast = core::cell::RefCell::new(BsnAst(parse_world));

    let lexer = Lexer::new(bsn_text);
    let patches_id = TopLevelPatchesParser::new()
        .parse(&ast, lexer)
        .map_err(|e| format!("BSN asset parse error: {e:?}"))?;

    let bsn_ast = ast.into_inner();
    let root_entries = unwrap_roots(&bsn_ast, patches_id)?;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>().cloned();

    let mut results = Vec::new();

    for entry_id in root_entries {
        let Some(patches) = bsn_ast.0.get::<BsnPatches>(entry_id) else {
            continue;
        };

        let mut name: Option<String> = None;

        for &patch_id in &patches.0 {
            let Some(patch) = bsn_ast.0.get::<BsnPatch>(patch_id) else {
                continue;
            };
            if let BsnPatch::Name(n, _) = patch {
                name = Some(n.clone());
            }
        }

        let Some(name) = name else { continue };

        for &patch_id in &patches.0 {
            let Some(patch) = bsn_ast.0.get::<BsnPatch>(patch_id) else {
                continue;
            };
            match patch {
                BsnPatch::Struct(bsn_struct) => {
                    let type_path = bsn_struct.0.as_path();
                    if let Some(handle) = create_asset_from_struct(
                        world,
                        &type_path,
                        &bsn_struct.1,
                        &bsn_ast,
                        &reg,
                        asset_server.as_ref(),
                    ) {
                        results.push((name.clone(), handle));
                    }
                }
                BsnPatch::Var(var) => {
                    let type_path = var.0.as_path();
                    if let Some(handle) = create_default_asset(world, &type_path, &reg) {
                        results.push((name.clone(), handle));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(results)
}

fn unwrap_roots(ast: &BsnAst, patches_id: Entity) -> Result<Vec<Entity>, String> {
    let Some(patches) = ast.0.get::<BsnPatches>(patches_id) else {
        return Err("No top-level patches found".to_string());
    };
    if patches.0.len() == 1 {
        if let Some(BsnPatch::Relation(relation)) = ast.0.get::<BsnPatch>(patches.0[0]) {
            return Ok(relation.1.clone());
        }
    }
    Ok(vec![patches_id])
}

fn create_asset_from_struct(
    world: &mut World,
    type_path: &str,
    fields: &[crate::dynamic_bsn::BsnField],
    ast: &BsnAst,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<bevy_asset::UntypedHandle> {
    let registration = registry.get_with_type_path(type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let mut value = reflect_default.default();

    if let ReflectMut::Struct(s) = value.reflect_mut() {
        let struct_info = registration.type_info().as_struct().ok()?;
        for field in fields {
            let Some(field_info) = struct_info.field(&field.0) else {
                continue;
            };
            let Some(expr) = ast.0.get::<BsnExpr>(field.1) else {
                continue;
            };
            apply_expr_to_field(
                s,
                &field.0,
                expr,
                field_info.ty().id(),
                registry,
                asset_server,
                ast,
            );
        }
    }

    let reflect_asset = registration.data::<bevy_asset::ReflectAsset>()?;
    Some(reflect_asset.add(world, value.as_partial_reflect()))
}

fn create_default_asset(
    world: &mut World,
    type_path: &str,
    registry: &TypeRegistry,
) -> Option<bevy_asset::UntypedHandle> {
    let registration = registry.get_with_type_path(type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let value = reflect_default.default();
    let reflect_asset = registration.data::<bevy_asset::ReflectAsset>()?;
    Some(reflect_asset.add(world, value.as_partial_reflect()))
}

fn apply_expr_to_field(
    target_struct: &mut dyn bevy_reflect::structs::Struct,
    field_name: &str,
    expr: &BsnExpr,
    expected_type_id: core::any::TypeId,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    ast: &BsnAst,
) {
    let Some(target) = target_struct.field_mut(field_name) else {
        return;
    };

    match expr {
        BsnExpr::FloatLit(f) => {
            if expected_type_id == core::any::TypeId::of::<f32>() {
                target.apply(&(*f as f32));
            } else if expected_type_id == core::any::TypeId::of::<f64>() {
                target.apply(f);
            }
        }
        BsnExpr::IntLit(i) => {
            macro_rules! try_int {
                ($($t:ty),*) => {
                    $(if expected_type_id == core::any::TypeId::of::<$t>() {
                        target.apply(&(*i as $t));
                        return;
                    })*
                };
            }
            try_int!(i8, u8, i16, u16, i32, u32, i64, u64, usize, isize);
        }
        BsnExpr::BoolLit(b) => {
            target.apply(b);
        }
        BsnExpr::StringLit(s) => {
            if let Some(asset_server) = asset_server {
                if let Some(concrete) = target.try_as_reflect() {
                    let type_id = concrete.reflect_type_info().type_id();
                    if registry
                        .get_type_data::<bevy_asset::ReflectHandle>(type_id)
                        .is_some()
                    {
                        let handle =
                            asset_server.load_untyped(AssetPath::parse(s).into_owned());
                        target.apply(handle.as_partial_reflect());
                        return;
                    }
                }
            }
            if expected_type_id == core::any::TypeId::of::<String>() {
                target.apply(&s.clone());
            }
        }
        BsnExpr::Struct(bsn_struct) => {
            let type_path = bsn_struct.0.as_path();
            if let Some(registration) = registry.get_with_type_path(&type_path) {
                if let Ok(struct_info) = registration.type_info().as_struct() {
                    if let ReflectMut::Struct(s) = target.reflect_mut() {
                        for field in &bsn_struct.1 {
                            if let Some(fi) = struct_info.field(&field.0) {
                                if let Some(expr) = ast.0.get::<BsnExpr>(field.1) {
                                    apply_expr_to_field(
                                        s,
                                        &field.0,
                                        expr,
                                        fi.ty().id(),
                                        registry,
                                        asset_server,
                                        ast,
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
}
