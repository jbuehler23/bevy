use crate::{PatchContext, Scene, SceneList, SceneListPatch, ScenePatch, ScenePatchInstance};
use bevy_asset::{AssetEvent, AssetServer, Assets, Handle};
use bevy_ecs::{
    message::MessageCursor,
    prelude::*,
    relationship::Relationship,
    template::{EntityScopes, ScopedEntities, TemplateContext},
};
use bevy_platform::collections::HashMap;
use std::sync::Arc;

pub trait SpawnScene {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_>;
    fn spawn_scene_immediate<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_>;
}

pub trait SpawnRelatedScenes {
    fn spawn_related_scenes<T: RelationshipTarget>(self, scenes: impl SceneList) -> Self;
}

impl SpawnScene for World {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_> {
        let assets = self.resource::<AssetServer>();
        let patch = ScenePatch::load(assets, scene);
        let handle = assets.add(patch);
        self.spawn(ScenePatchInstance(handle))
    }

    // TODO: this exists for easier benchmarking. consider removing it?
    fn spawn_scene_immediate<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_> {
        let assets = self.resource::<AssetServer>();
        let patch = ScenePatch::load(assets, scene);
        // TODO: return error
        if patch
            .dependencies
            .iter()
            .any(|h| !assets.dependency_load_state(h).is_loaded())
        {
            panic!("This scene has unloaded dependencies!");
        }

        let (resolved, entity_scopes) =
            patch.resolve(assets, self.resource::<Assets<ScenePatch>>());
        let mut entity = self.spawn_empty();
        let mut scoped_entities = ScopedEntities::new(entity_scopes.entity_count());
        resolved
            .apply(&mut TemplateContext::new(
                &mut entity,
                &mut scoped_entities,
                &entity_scopes,
            ))
            .unwrap();
        entity
    }
}

impl SpawnRelatedScenes for EntityWorldMut<'_> {
    fn spawn_related_scenes<T: RelationshipTarget>(mut self, scenes: impl SceneList) -> Self {
        let assets = self.resource::<AssetServer>();
        let patch = SceneListPatch::load(assets, scenes);
        let handle = assets.add(patch);
        let entity = self.id();
        self.resource_mut::<NewScenes>().scene_entities.push((
            SceneListSpawn {
                entity,
                insert: |entity, target| {
                    entity.insert(
                        <<T as RelationshipTarget>::Relationship as Relationship>::from(target),
                    );
                },
            },
            handle,
        ));
        self
    }
}

impl SpawnRelatedScenes for EntityCommands<'_> {
    fn spawn_related_scenes<T: RelationshipTarget>(mut self, scenes: impl SceneList) -> Self {
        self.queue(move |entity: EntityWorldMut| {
            entity.spawn_related_scenes::<T>(scenes);
        });

        self
    }
}

pub trait CommandsSpawnScene {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityCommands<'_>;
}

impl<'w, 's> CommandsSpawnScene for Commands<'w, 's> {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityCommands<'_> {
        let mut entity_commands = self.spawn_empty();
        let id = entity_commands.id();
        entity_commands.commands().queue(move |world: &mut World| {
            let assets = world.resource::<AssetServer>();
            let patch = ScenePatch::load(assets, scene);
            let handle = assets.add(patch);
            if let Ok(mut entity) = world.get_entity_mut(id) {
                entity.insert(ScenePatchInstance(handle));
            }
        });
        entity_commands
    }
}

pub fn resolve_scene_patches(
    mut events: MessageReader<AssetEvent<ScenePatch>>,
    mut list_events: MessageReader<AssetEvent<SceneListPatch>>,
    assets: Res<AssetServer>,
    mut patches: ResMut<Assets<ScenePatch>>,
    mut list_patches: ResMut<Assets<SceneListPatch>>,
) {
    for event in events.read() {
        match *event {
            // TODO: handle modified?
            AssetEvent::LoadedWithDependencies { id } => {
                // TODO: real error handling
                let patch = patches.get(id).unwrap();
                let resolved = patch.resolve(&assets, &patches);
                let mut patch = patches.get_mut(id).unwrap();
                patch.resolved = Some(Arc::new(resolved));
            }
            _ => {}
        }
    }
    for event in list_events.read() {
        match *event {
            // TODO: handle modified?
            AssetEvent::LoadedWithDependencies { id } => {
                let mut list_patch = list_patches.get_mut(id).unwrap();
                let mut entity_scopes = EntityScopes::default();
                let mut scenes = Vec::new();
                // TODO: real error handling
                list_patch.patch.patch_list(
                    &mut PatchContext {
                        assets: &assets,
                        patches: &patches,
                        current_scope: entity_scopes.add_scope(),
                        entity_scopes: &mut entity_scopes,
                        inherited: None,
                    },
                    &mut scenes,
                );
                list_patch.resolved = Some(scenes);
                list_patch.entity_scopes = Some(entity_scopes);
            }
            _ => {}
        }
    }
}

#[derive(Resource, Default)]
pub struct QueuedScenes {
    waiting_entities: HashMap<Handle<ScenePatch>, Vec<Entity>>,
    waiting_list_entities: HashMap<Handle<SceneListPatch>, Vec<SceneListSpawn>>,
}

struct SceneListSpawn {
    entity: Entity,
    insert: fn(&mut EntityWorldMut, target: Entity),
}

#[derive(Resource, Default)]
pub struct NewScenes {
    entities: Vec<Entity>,
    scene_entities: Vec<(SceneListSpawn, Handle<SceneListPatch>)>,
}

pub fn on_add_scene_patch_instance(
    add: On<Add, ScenePatchInstance>,
    mut new_scenes: ResMut<NewScenes>,
) {
    new_scenes.entities.push(add.entity);
}

pub fn spawn_queued(
    world: &mut World,
    handles: &mut QueryState<&ScenePatchInstance>,
    mut reader: Local<MessageCursor<AssetEvent<ScenePatch>>>,
    mut list_reader: Local<MessageCursor<AssetEvent<SceneListPatch>>>,
) {
    world.resource_scope(|world, mut list_patches: Mut<Assets<SceneListPatch>>| {
        world.resource_scope(|world, mut queued: Mut<QueuedScenes>| {
            world.resource_scope(|world, events: Mut<Messages<AssetEvent<ScenePatch>>>| {
                world.resource_scope(
                    |world, list_events: Mut<Messages<AssetEvent<SceneListPatch>>>| {
                        loop {
                            let mut new_scenes = world.resource_mut::<NewScenes>();
                            if new_scenes.entities.is_empty() {
                                break;
                            }
                            for entity in core::mem::take(&mut new_scenes.entities) {
                                if let Ok(handle) = handles.get(world, entity).map(|h| &h.0) {
                                    let patches = world.resource::<Assets<ScenePatch>>();
                                    if let Some(resolved) =
                                        patches.get(handle).and_then(|p| p.resolved.clone())
                                    {
                                        let (scene, entity_scopes) = &*resolved;
                                        let mut entity_mut = world.get_entity_mut(entity).unwrap();
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut entity_mut,
                                                &mut ScopedEntities::new(
                                                    entity_scopes.entity_count(),
                                                ),
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    } else {
                                        let entities = queued
                                            .waiting_entities
                                            .entry(handle.clone())
                                            .or_default();
                                        entities.push(entity);
                                    }
                                }
                            }
                        }
                        loop {
                            let mut new_scenes = world.resource_mut::<NewScenes>();
                            if new_scenes.scene_entities.is_empty() {
                                break;
                            }
                            for (scene_list_spawn, handle) in
                                core::mem::take(&mut new_scenes.scene_entities)
                            {
                                if let Some((Some(resolved_scenes), Some(entity_scopes))) =
                                    list_patches.get_mut(&handle).map(|p| {
                                        let p = p.into_inner();
                                        (p.resolved.as_mut(), p.entity_scopes.as_ref())
                                    })
                                {
                                    for scene in resolved_scenes {
                                        let mut child_entity = world.spawn_empty();
                                        (scene_list_spawn.insert)(
                                            &mut child_entity,
                                            scene_list_spawn.entity,
                                        );
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut child_entity,
                                                &mut ScopedEntities::new(
                                                    entity_scopes.entity_count(),
                                                ),
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    }
                                } else {
                                    let entities =
                                        queued.waiting_list_entities.entry(handle).or_default();
                                    entities.push(scene_list_spawn);
                                }
                            }
                        }

                        for event in reader.read(&events) {
                            let patches = world.resource::<Assets<ScenePatch>>();
                            if let AssetEvent::LoadedWithDependencies { id } = event
                                && let Some(resolved) =
                                    patches.get(*id).and_then(|p| p.resolved.clone())
                                && let Some(entities) = queued.waiting_entities.remove(id)
                            {
                                let (scene, entity_scopes) = &*resolved;
                                for entity in entities {
                                    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut entity_mut,
                                                &mut ScopedEntities::new(
                                                    entity_scopes.entity_count(),
                                                ),
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    }
                                }
                            }
                        }
                        for event in list_reader.read(&list_events) {
                            if let AssetEvent::LoadedWithDependencies { id } = event
                                && let Some((Some(resolved_scenes), Some(entity_scopes))) =
                                    list_patches.get_mut(*id).map(|p| {
                                        let p = p.into_inner();
                                        (p.resolved.as_mut(), p.entity_scopes.as_ref())
                                    })
                                && let Some(scene_list_spawns) =
                                    queued.waiting_list_entities.remove(id)
                            {
                                for scene_list_spawn in scene_list_spawns {
                                    for scene in resolved_scenes.iter_mut() {
                                        let mut child_entity = world.spawn_empty();
                                        (scene_list_spawn.insert)(
                                            &mut child_entity,
                                            scene_list_spawn.entity,
                                        );
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut child_entity,
                                                &mut ScopedEntities::new(
                                                    entity_scopes.entity_count(),
                                                ),
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    }
                                }
                            }
                        }
                    },
                );
            });
        });
    });
}
