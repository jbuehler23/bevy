//! BSN scene writer: serialize ECS World and assets to `.bsn` text.

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use core::any::TypeId;
use core::fmt::Write;

use bevy_asset::{AssetServer, ReflectHandle};
use bevy_ecs::{
    hierarchy::ChildOf,
    name::Name,
    prelude::*,
    reflect::{AppTypeRegistry, ReflectComponent},
};
use bevy_reflect::{PartialReflect, ReflectRef, TypeRegistry};

const SKIP_TYPE_NAMES: &[&str] = &[
    "bevy_transform::components::global_transform::GlobalTransform",
    "bevy_camera::visibility::InheritedVisibility",
    "bevy_camera::visibility::ViewVisibility",
    "bevy_ecs::hierarchy::ChildOf",
    "bevy_ecs::hierarchy::Children",
];

/// Serialize all named scene entities from the world to BSN text.
pub fn serialize_to_bsn(world: &World) -> String {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();

    let skip_ids: BTreeSet<TypeId> = SKIP_TYPE_NAMES
        .iter()
        .filter_map(|name| reg.get_with_type_path(name).map(|r| r.type_id()))
        .collect();

    let scene_entities: Vec<Entity> = world
        .iter_entities()
        .filter(|e| e.contains::<Name>())
        .map(|e| e.id())
        .collect();
    let entity_set: BTreeSet<Entity> = scene_entities.iter().copied().collect();

    let mut roots = Vec::new();
    let mut children_map: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
    for &entity in &scene_entities {
        let parent = world
            .get::<ChildOf>(entity)
            .map(|c| c.parent())
            .filter(|p| entity_set.contains(p));
        match parent {
            Some(p) => children_map.entry(p).or_default().push(entity),
            None => roots.push(entity),
        }
    }

    let mut out = String::new();
    if roots.len() <= 1 {
        for &root in &roots {
            emit_entity(world, root, &reg, asset_server, &skip_ids, &children_map, 0, &mut out);
        }
    } else {
        emit_children_block(&roots, world, &reg, asset_server, &skip_ids, &children_map, 0, &mut out);
    }
    out
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
///         }
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
        let Some(registration) = reg.get(*type_id) else { continue };
        let Some(reflect_asset) = registration.data::<bevy_asset::ReflectAsset>() else { continue };
        let Some(asset_data) = reflect_asset.get(world, *asset_id) else { continue };

        let type_path = registration.type_info().type_path_table().path();
        let default_data = registration
            .data::<bevy_reflect::prelude::ReflectDefault>()
            .map(|rd| rd.default());

        let mut entry = String::new();
        emit_name(name, 1, &mut entry);

        if let ReflectRef::Struct(s) = asset_data.reflect_ref() {
            let default_struct = default_data.as_ref().and_then(|d| match d.reflect_ref() {
                ReflectRef::Struct(ds) => Some(ds),
                _ => None,
            });

            let fields = collect_non_default_fields(s, default_struct, &reg, asset_server);
            emit_struct_fields(type_path, &fields, 1, &mut entry);
        } else {
            indent_write(&mut entry, 1, &format!("{type_path}\n"));
        }

        entries.push((name.clone(), entry));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    wrap_children_block(&entries)
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
    if let Some(name) = world.get::<Name>(entity) {
        emit_name(name.as_str(), indent, out);
    }

    let entity_ref = world.entity(entity);
    for component_id in entity_ref.archetype().iter_components() {
        let Some(info) = world.components().get_info(component_id) else { continue };
        let Some(type_id) = info.type_id() else { continue };
        if type_id == TypeId::of::<Name>() || skip_ids.contains(&type_id) {
            continue;
        }
        let Some(registration) = registry.get(type_id) else { continue };
        let type_path = registration.type_info().type_path_table().path();
        if type_path.starts_with("jackdaw") && !type_path.starts_with("jackdaw_jsn") {
            continue;
        }
        let Some(reflect_component) = registration.data::<ReflectComponent>() else { continue };
        let Some(reflected) = reflect_component.reflect(entity_ref) else { continue };

        emit_component(reflected, type_path, registry, asset_server, indent, out);
    }

    if let Some(children) = children_map.get(&entity) {
        emit_children_block(children, world, registry, asset_server, skip_ids, children_map, indent, out);
    }
}

fn emit_children_block(
    children: &[Entity],
    world: &World,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    skip_ids: &BTreeSet<TypeId>,
    children_map: &BTreeMap<Entity, Vec<Entity>>,
    indent: usize,
    out: &mut String,
) {
    indent_write(out, indent, "bevy_ecs::hierarchy::Children [\n");
    for (i, &child) in children.iter().enumerate() {
        emit_entity(world, child, registry, asset_server, skip_ids, children_map, indent + 1, out);
        if i + 1 < children.len() {
            indent_write(out, indent + 1, ",\n");
        }
    }
    indent_write(out, indent, "]\n");
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
        ReflectRef::Struct(s) if s.field_len() > 0 => {
            indent_write(out, indent, &format!("{type_path} {{\n"));
            for i in 0..s.field_len() {
                indent_write(out, indent + 1, &format!("{}: ", s.name_at(i).unwrap()));
                emit_value(s.field_at(i).unwrap(), registry, asset_server, indent + 1, true, out);
                writeln!(out, ",").unwrap();
            }
            indent_write(out, indent, "}\n");
        }
        ReflectRef::TupleStruct(ts) => {
            indent_write(out, indent, &format!("{type_path}("));
            for i in 0..ts.field_len() {
                if i > 0 { write!(out, ", ").unwrap(); }
                emit_value(ts.field(i).unwrap(), registry, asset_server, indent, false, out);
            }
            writeln!(out, ")").unwrap();
        }
        ReflectRef::Enum(e) => {
            indent_write(out, indent, &format!("{type_path}::{}\n", e.variant_name()));
        }
        _ => {
            indent_write(out, indent, &format!("{type_path}\n"));
        }
    }
}

/// Emit a value. `multiline` controls whether structs/lists use indented multiline format.
fn emit_value(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
    indent: usize,
    multiline: bool,
    out: &mut String,
) {
    // Primitives
    if let Some(v) = value.try_downcast_ref::<f32>() { return write_float(*v as f64, out); }
    if let Some(v) = value.try_downcast_ref::<f64>() { return write_float(*v, out); }
    if let Some(v) = value.try_downcast_ref::<bool>() { return write!(out, "{v}").unwrap(); }
    if let Some(v) = value.try_downcast_ref::<String>() {
        return write!(out, "\"{}\"", escape_string(v)).unwrap();
    }
    macro_rules! try_int {
        ($($t:ty),*) => { $(if let Some(v) = value.try_downcast_ref::<$t>() { return write!(out, "{v}").unwrap(); })* };
    }
    try_int!(i8, u8, i16, u16, i32, u32, i64, u64, isize, usize);

    // Handle<T> → asset path
    if let Some(path) = try_resolve_handle(value, registry, asset_server) {
        write!(out, "\"{}\"", escape_string(&path)).unwrap();
        return;
    }

    let tp = type_path_of(value);

    match value.reflect_ref() {
        ReflectRef::Struct(s) if s.field_len() > 0 => {
            if multiline {
                writeln!(out, "{tp} {{").unwrap();
                for i in 0..s.field_len() {
                    indent_write(out, indent + 1, &format!("{}: ", s.name_at(i).unwrap()));
                    emit_value(s.field_at(i).unwrap(), registry, asset_server, indent + 1, true, out);
                    writeln!(out, ",").unwrap();
                }
                indent_write(out, indent, "}");
            } else {
                write!(out, "{tp} {{ ").unwrap();
                for i in 0..s.field_len() {
                    if i > 0 { write!(out, ", ").unwrap(); }
                    write!(out, "{}: ", s.name_at(i).unwrap()).unwrap();
                    emit_value(s.field_at(i).unwrap(), registry, asset_server, indent, false, out);
                }
                write!(out, " }}").unwrap();
            }
        }
        ReflectRef::Struct(_) => write!(out, "{tp}").unwrap(),
        ReflectRef::TupleStruct(ts) => {
            write!(out, "{tp}(").unwrap();
            for i in 0..ts.field_len() {
                if i > 0 { write!(out, ", ").unwrap(); }
                emit_value(ts.field(i).unwrap(), registry, asset_server, indent, false, out);
            }
            write!(out, ")").unwrap();
        }
        ReflectRef::Enum(e) => write!(out, "{tp}::{}", e.variant_name()).unwrap(),
        ReflectRef::List(l) if l.len() > 0 && multiline => {
            writeln!(out, "[").unwrap();
            for i in 0..l.len() {
                indent_write(out, indent + 1, "");
                if let Some(item) = l.get(i) {
                    emit_value(item, registry, asset_server, indent + 1, true, out);
                }
                writeln!(out, "{}", if i + 1 < l.len() { "," } else { "" }).unwrap();
            }
            indent_write(out, indent, "]");
        }
        ReflectRef::List(l) => {
            write!(out, "[").unwrap();
            for i in 0..l.len() {
                if i > 0 { write!(out, ", ").unwrap(); }
                if let Some(item) = l.get(i) {
                    emit_value(item, registry, asset_server, indent, false, out);
                }
            }
            write!(out, "]").unwrap();
        }
        _ => write!(out, "\"<unsupported>\"").unwrap(),
    }
}

fn type_path_of(value: &dyn PartialReflect) -> &str {
    value
        .get_represented_type_info()
        .map(|i| i.type_path())
        .unwrap_or("unknown")
}

fn try_resolve_handle(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<String> {
    let asset_server = asset_server?;
    let concrete = value.try_as_reflect()?;
    let type_id = concrete.reflect_type_info().type_id();
    let reflect_handle = registry.get_type_data::<ReflectHandle>(type_id)?;
    let handle = reflect_handle.downcast_handle_untyped(concrete.as_any())?;
    asset_server.get_path(handle.id()).map(|p| p.to_string())
}

fn collect_non_default_fields(
    s: &dyn bevy_reflect::structs::Struct,
    default_struct: Option<&dyn bevy_reflect::structs::Struct>,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for i in 0..s.field_len() {
        let name = s.name_at(i).unwrap();
        let value = s.field_at(i).unwrap();

        if let Some(ds) = default_struct {
            if let Some(df) = ds.field(name) {
                if value.reflect_partial_eq(df).unwrap_or(false) {
                    continue;
                }
            }
        }

        // Try Handle → path
        if let Some(path) = try_resolve_handle(value, registry, asset_server) {
            fields.push((name.to_string(), format!("\"{}\"", escape_string(&path))));
            continue;
        }

        // Skip generic types the parser can't handle
        if let Some(ti) = value.get_represented_type_info() {
            if ti.type_path().contains('<') {
                continue;
            }
        }

        let mut val = String::new();
        emit_value(value, registry, asset_server, 2, false, &mut val);
        fields.push((name.to_string(), val));
    }
    fields
}

fn emit_struct_fields(type_path: &str, fields: &[(String, String)], indent: usize, out: &mut String) {
    if fields.is_empty() {
        indent_write(out, indent, &format!("{type_path}\n"));
    } else {
        indent_write(out, indent, &format!("{type_path} {{\n"));
        for (name, val) in fields {
            indent_write(out, indent + 1, &format!("{name}: {val},\n"));
        }
        indent_write(out, indent, "}\n");
    }
}

fn emit_name(name: &str, indent: usize, out: &mut String) {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
        indent_write(out, indent, &format!("#{name}\n"));
    } else {
        indent_write(out, indent, &format!("#\"{}\"\n", escape_string(name)));
    }
}

fn wrap_children_block(entries: &[(String, String)]) -> String {
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

fn indent_write(out: &mut String, indent: usize, text: &str) {
    for _ in 0..indent {
        out.push_str("    ");
    }
    out.push_str(text);
}

fn write_float(f: f64, out: &mut String) {
    if f.fract() == 0.0 { write!(out, "{f:.1}").unwrap(); }
    else { write!(out, "{f}").unwrap(); }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
