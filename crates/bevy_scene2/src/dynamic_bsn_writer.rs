//! BSN scene writer: serialize ECS World → `.bsn` text.
//!
//! Reflects all components on scene entities and emits BSN text format
//! compatible with [`DynamicBsnLoader`](super::dynamic_bsn::DynamicBsnLoader).

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use core::any::TypeId;
use core::fmt::Write;

use bevy_asset::{AssetServer, ReflectHandle};
use bevy_ecs::{
    hierarchy::{ChildOf, Children},
    name::Name,
    prelude::*,
    reflect::{AppTypeRegistry, ReflectComponent},
};
use bevy_reflect::{PartialReflect, ReflectRef, TypeRegistry};

/// Components that are derived/internal and should not be serialized.
const SKIP_TYPE_NAMES: &[&str] = &[
    "bevy_transform::components::global_transform::GlobalTransform",
    "bevy_camera::visibility::InheritedVisibility",
    "bevy_camera::visibility::ViewVisibility",
    "bevy_ecs::hierarchy::ChildOf",
    "bevy_ecs::hierarchy::Children",
];

/// Serialize scene entities from the world to BSN text.
///
/// `entities` should be the root + descendant entities to include.
/// Hierarchy is reconstructed from `ChildOf` relationships.
pub fn serialize_to_bsn(world: &World) -> String {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();

    // Build skip set from type names
    let skip_ids: BTreeSet<TypeId> = SKIP_TYPE_NAMES
        .iter()
        .filter_map(|name| reg.get_with_type_path(name).map(|r| r.type_id()))
        .collect();

    // Collect all named entities (scene entities)
    let scene_entities: Vec<Entity> = world
        .iter_entities()
        .filter(|e| e.contains::<Name>())
        .map(|e| e.id())
        .collect();

    let entity_set: BTreeSet<Entity> = scene_entities.iter().copied().collect();

    // Build parent → children map and identify roots
    let mut roots = Vec::new();
    let mut children_map: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
    for &entity in &scene_entities {
        let parent = world
            .get::<ChildOf>(entity)
            .map(|c| c.parent())
            .filter(|p| entity_set.contains(p));
        if let Some(parent) = parent {
            children_map.entry(parent).or_default().push(entity);
        } else {
            roots.push(entity);
        }
    }

    // Emit BSN
    let mut out = String::new();

    if roots.len() <= 1 {
        for &root in &roots {
            emit_entity(world, root, &reg, asset_server, &skip_ids, &children_map, 0, &mut out);
        }
    } else {
        writeln!(out, "bevy_ecs::hierarchy::Children [").unwrap();
        for (i, &root) in roots.iter().enumerate() {
            emit_entity(world, root, &reg, asset_server, &skip_ids, &children_map, 1, &mut out);
            if i + 1 < roots.len() {
                write_indent(1, &mut out);
                out.push_str(",\n");
            }
        }
        writeln!(out, "]").unwrap();
    }

    out
}

fn emit_entity(
    world: &World,
    entity: Entity,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    skip_ids: &BTreeSet<TypeId>,
    children_map: &BTreeMap<Entity, Vec<Entity>>,
    indent: usize,
    out: &mut String,
) {
    // Name
    if let Some(name) = world.get::<Name>(entity) {
        write_indent(indent, out);
        let name_str = name.as_str();
        if name_str
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name_str.is_empty()
        {
            writeln!(out, "#{name_str}").unwrap();
        } else {
            writeln!(out, "#\"{}\"", escape_string(name_str)).unwrap();
        }
    }

    // Components via reflection
    let entity_ref = world.entity(entity);
    for component_id in entity_ref.archetype().iter_components() {
        let Some(info) = world.components().get_info(component_id) else {
            continue;
        };
        let Some(type_id) = info.type_id() else {
            continue;
        };

        // Skip Name (emitted above), internal components, and non-scene types
        if type_id == TypeId::of::<Name>() || skip_ids.contains(&type_id) {
            continue;
        }

        let Some(registration) = registry.get(type_id) else {
            continue;
        };

        let type_path = registration.type_info().type_path_table().path();

        // Skip jackdaw-internal components if present
        if type_path.starts_with("jackdaw") && !type_path.starts_with("jackdaw_jsn") {
            continue;
        }

        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(reflected) = reflect_component.reflect(entity_ref) else {
            continue;
        };

        emit_component(reflected, type_path, registry, asset_server, indent, out);
    }

    // Children relation
    if let Some(children) = children_map.get(&entity) {
        write_indent(indent, out);
        writeln!(out, "bevy_ecs::hierarchy::Children [").unwrap();
        for (i, child) in children.iter().enumerate() {
            emit_entity(world, *child, registry, asset_server, skip_ids, children_map, indent + 1, out);
            if i + 1 < children.len() {
                write_indent(indent + 1, out);
                out.push_str(",\n");
            }
        }
        write_indent(indent, out);
        writeln!(out, "]").unwrap();
    }
}

fn emit_component(
    reflected: &dyn PartialReflect,
    type_path: &str,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    indent: usize,
    out: &mut String,
) {
    match reflected.reflect_ref() {
        ReflectRef::Struct(s) => {
            // Check if all fields match default → emit bare type
            let has_fields = s.field_len() > 0;
            if !has_fields {
                write_indent(indent, out);
                writeln!(out, "{type_path}").unwrap();
                return;
            }

            // Emit struct with fields
            write_indent(indent, out);
            writeln!(out, "{type_path} {{").unwrap();
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap();
                let value = s.field_at(i).unwrap();
                write_indent(indent + 1, out);
                write!(out, "{name}: ").unwrap();
                emit_value_multiline(value, registry, asset_server, indent + 1, out);
                writeln!(out, ",").unwrap();
            }
            write_indent(indent, out);
            writeln!(out, "}}").unwrap();
        }
        ReflectRef::TupleStruct(ts) => {
            write_indent(indent, out);
            write!(out, "{type_path}(").unwrap();
            for i in 0..ts.field_len() {
                if i > 0 {
                    write!(out, ", ").unwrap();
                }
                let value = ts.field(i).unwrap();
                emit_value_inline(value, registry, asset_server, out);
            }
            writeln!(out, ")").unwrap();
        }
        ReflectRef::Enum(e) => {
            write_indent(indent, out);
            writeln!(out, "{type_path}::{}", e.variant_name()).unwrap();
        }
        _ => {
            // Bare type
            write_indent(indent, out);
            writeln!(out, "{type_path}").unwrap();
        }
    }
}

/// Emit a reflected value inline (no newlines).
fn emit_value_inline(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    out: &mut String,
) {
    // Primitives
    if let Some(v) = value.try_downcast_ref::<f32>() {
        return write_float(*v as f64, out);
    }
    if let Some(v) = value.try_downcast_ref::<f64>() {
        return write_float(*v, out);
    }
    if let Some(v) = value.try_downcast_ref::<bool>() {
        return write!(out, "{v}").unwrap();
    }
    if let Some(v) = value.try_downcast_ref::<String>() {
        return write!(out, "\"{}\"", escape_string(v)).unwrap();
    }
    // Integer types
    macro_rules! try_int {
        ($($t:ty),*) => {
            $(if let Some(v) = value.try_downcast_ref::<$t>() {
                return write!(out, "{v}").unwrap();
            })*
        };
    }
    try_int!(i8, u8, i16, u16, i32, u32, i64, u64, isize, usize);

    // Handle<T> → asset path
    if let Some(asset_server) = asset_server {
        if let Some(concrete) = value.try_as_reflect() {
            let type_id = concrete.reflect_type_info().type_id();
            if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
                if let Some(handle) = reflect_handle.downcast_handle_untyped(concrete.as_any()) {
                    if let Some(path) = asset_server.get_path(handle.id()) {
                        write!(out, "\"{}\"", escape_string(&path.to_string())).unwrap();
                        return;
                    }
                }
                write!(out, "\"\"").unwrap();
                return;
            }
        }
    }

    // Struct
    if let ReflectRef::Struct(s) = value.reflect_ref() {
        let tp = value
            .get_represented_type_info()
            .map(|i| i.type_path())
            .unwrap_or("unknown");
        if s.field_len() == 0 {
            write!(out, "{tp}").unwrap();
        } else {
            write!(out, "{tp} {{ ").unwrap();
            for i in 0..s.field_len() {
                if i > 0 {
                    write!(out, ", ").unwrap();
                }
                write!(out, "{}: ", s.name_at(i).unwrap()).unwrap();
                emit_value_inline(s.field_at(i).unwrap(), registry, asset_server, out);
            }
            write!(out, " }}").unwrap();
        }
        return;
    }

    // TupleStruct
    if let ReflectRef::TupleStruct(ts) = value.reflect_ref() {
        let tp = value
            .get_represented_type_info()
            .map(|i| i.type_path())
            .unwrap_or("unknown");
        write!(out, "{tp}(").unwrap();
        for i in 0..ts.field_len() {
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            emit_value_inline(ts.field(i).unwrap(), registry, asset_server, out);
        }
        write!(out, ")").unwrap();
        return;
    }

    // Enum
    if let ReflectRef::Enum(e) = value.reflect_ref() {
        let tp = value
            .get_represented_type_info()
            .map(|i| i.type_path())
            .unwrap_or("unknown");
        write!(out, "{tp}::{}", e.variant_name()).unwrap();
        return;
    }

    // List
    if let ReflectRef::List(l) = value.reflect_ref() {
        write!(out, "[").unwrap();
        for i in 0..l.len() {
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            if let Some(item) = l.get(i) {
                emit_value_inline(item, registry, asset_server, out);
            }
        }
        write!(out, "]").unwrap();
        return;
    }

    // Fallback
    write!(out, "\"<unsupported>\"").unwrap();
}

/// Emit a reflected value, using multiline format for nested structs and lists.
fn emit_value_multiline(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    indent: usize,
    out: &mut String,
) {
    // Struct with fields → multiline
    if let ReflectRef::Struct(s) = value.reflect_ref() {
        if s.field_len() > 0 {
            let tp = value
                .get_represented_type_info()
                .map(|i| i.type_path())
                .unwrap_or("unknown");
            writeln!(out, "{tp} {{").unwrap();
            for i in 0..s.field_len() {
                write_indent(indent + 1, out);
                write!(out, "{}: ", s.name_at(i).unwrap()).unwrap();
                emit_value_multiline(s.field_at(i).unwrap(), registry, asset_server, indent + 1, out);
                writeln!(out, ",").unwrap();
            }
            write_indent(indent, out);
            write!(out, "}}").unwrap();
            return;
        }
    }

    // List with items → multiline
    if let ReflectRef::List(l) = value.reflect_ref() {
        if l.len() > 0 {
            writeln!(out, "[").unwrap();
            for i in 0..l.len() {
                write_indent(indent + 1, out);
                if let Some(item) = l.get(i) {
                    emit_value_multiline(item, registry, asset_server, indent + 1, out);
                }
                if i + 1 < l.len() {
                    writeln!(out, ",").unwrap();
                } else {
                    writeln!(out).unwrap();
                }
            }
            write_indent(indent, out);
            write!(out, "]").unwrap();
            return;
        }
    }

    // Everything else → inline
    emit_value_inline(value, registry, asset_server, out);
}

fn write_float(f: f64, out: &mut String) {
    if f.fract() == 0.0 {
        write!(out, "{f:.1}").unwrap();
    } else {
        write!(out, "{f}").unwrap();
    }
}

fn write_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("    ");
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Serialize named assets to a BSN catalog file.
///
/// Each `(name, type_id, asset_id)` triple is reflected from the corresponding
/// `Assets<T>` store, compared to its default, and emitted with only non-default
/// fields. Handle fields are resolved to asset path strings.
///
/// Example output:
///
///     bevy_ecs::hierarchy::Children [
///         #ground06
///         bevy_pbr::pbr_material::StandardMaterial {
///             base_color_texture: "ground06.png",
///         },
///         #rock058
///         bevy_pbr::pbr_material::StandardMaterial { ... }
///     ]
pub fn serialize_assets_to_bsn(
    world: &World,
    assets: &[(String, TypeId, bevy_asset::UntypedAssetId)],
) -> String {
    if assets.is_empty() {
        return String::new();
    }

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();

    let mut entries: Vec<(String, String)> = Vec::new();

    for (name, type_id, asset_id) in assets {
        let Some(registration) = reg.get(*type_id) else {
            continue;
        };
        let Some(reflect_asset) = registration.data::<bevy_asset::ReflectAsset>() else {
            continue;
        };
        let Some(asset_data) = reflect_asset.get(world, *asset_id) else {
            continue;
        };

        let type_path = registration.type_info().type_path_table().path();

        // Get default for comparison
        let default_data = registration
            .data::<bevy_reflect::prelude::ReflectDefault>()
            .map(|rd| rd.default());

        // Emit only non-default fields
        let mut entry = String::new();

        // Name
        if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
            writeln!(entry, "    #{name}").unwrap();
        } else {
            writeln!(entry, "    #\"{}\"", escape_string(name)).unwrap();
        }

        // Component fields
        if let ReflectRef::Struct(s) = asset_data.reflect_ref() {
            let default_struct = default_data
                .as_ref()
                .and_then(|d| match d.reflect_ref() {
                    ReflectRef::Struct(ds) => Some(ds),
                    _ => None,
                });

            let mut fields: Vec<(String, String)> = Vec::new();
            for i in 0..s.field_len() {
                let field_name = s.name_at(i).unwrap();
                let field_value = s.field_at(i).unwrap();

                // Skip if same as default
                if let Some(default_struct) = &default_struct {
                    if let Some(default_field) = default_struct.field(field_name) {
                        if field_value
                            .reflect_partial_eq(default_field)
                            .unwrap_or(false)
                        {
                            continue;
                        }
                    }
                }

                // Skip Handle fields that have no asset path (runtime-only handles)
                if let Some(asset_server) = asset_server {
                    if let Some(concrete) = field_value.try_as_reflect() {
                        let ftype_id = concrete.reflect_type_info().type_id();
                        if let Some(reflect_handle) =
                            reg.get_type_data::<bevy_asset::ReflectHandle>(ftype_id)
                        {
                            if let Some(handle) =
                                reflect_handle.downcast_handle_untyped(concrete.as_any())
                            {
                                if let Some(path) = asset_server.get_path(handle.id()) {
                                    let mut val = String::new();
                                    write!(val, "\"{}\"", escape_string(&path.to_string()))
                                        .unwrap();
                                    fields.push((field_name.to_string(), val));
                                }
                                // Skip handles without paths (don't emit empty strings)
                                continue;
                            }
                        }
                    }
                }

                // Skip unsupported types (Option<Handle<T>>, generic enums with <>)
                if let Some(type_info) = field_value.get_represented_type_info() {
                    let field_type_path = type_info.type_path();
                    if field_type_path.contains('<') {
                        continue; // BSN parser can't handle generic type paths
                    }
                }

                let mut val = String::new();
                emit_value_inline(field_value, &reg, asset_server, &mut val);
                fields.push((field_name.to_string(), val));
            }

            if fields.is_empty() {
                writeln!(entry, "    {type_path}").unwrap();
            } else {
                writeln!(entry, "    {type_path} {{").unwrap();
                for (fname, fval) in &fields {
                    writeln!(entry, "        {fname}: {fval},").unwrap();
                }
                writeln!(entry, "    }}").unwrap();
            }
        } else {
            writeln!(entry, "    {type_path}").unwrap();
        }

        entries.push((name.clone(), entry));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::from("bevy_ecs::hierarchy::Children [\n");
    for (i, (_, entry)) in entries.iter().enumerate() {
        out.push_str(entry);
        if i + 1 < entries.len() {
            out.push_str("    ,\n");
        }
    }
    out.push_str("]\n");
    out
}
