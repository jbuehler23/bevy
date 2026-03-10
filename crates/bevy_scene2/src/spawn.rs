use crate::{Scene, SceneList, SceneListPatch, ScenePatch, ScenePatchInstance};
use bevy_asset::{AssetEvent, AssetServer, Assets, Handle};
use bevy_ecs::{
    message::MessageCursor,
    prelude::*,
    relationship::Relationship,
    template::{ScopedEntities, TemplateContext},
};
use bevy_platform::collections::HashMap;
use std::sync::Arc;

pub trait SpawnScene {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_>;
    fn spawn_scene_immediate<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_>;
    fn spawn_scene_list<L: SceneList>(&mut self, scenes: L);
    // PERF: ideally this is an iterator
    fn spawn_scene_list_immediate<L: SceneList>(&mut self, scenes: L) -> Vec<Entity>;
}

impl SpawnScene for World {
    fn spawn_scene<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_> {
        let assets = self.resource::<AssetServer>();
        let patch = ScenePatch::load(assets, scene);
        let handle = assets.add(patch);
        self.spawn(ScenePatchInstance(handle))
    }

    fn spawn_scene_immediate<S: Scene>(&mut self, scene: S) -> EntityWorldMut<'_> {
        let assets = self.resource::<AssetServer>();
        let patch = ScenePatch::load(assets, scene);
        // TODO: return error
        if patch
            .dependencies
            .iter()
            .any(|h| !assets.dependency_load_state(h).is_loaded())
        {
            panic!("This Scene has unloaded dependencies!");
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

    fn spawn_scene_list<L: SceneList>(&mut self, scenes: L) {
        let assets = self.resource::<AssetServer>();
        let patch = SceneListPatch::load(assets, scenes);
        let handle = assets.add(patch);
        self.resource_mut::<NewScenes>()
            .scene_list_spawns
            .push(handle);
    }

    fn spawn_scene_list_immediate<L: SceneList>(&mut self, scenes: L) -> Vec<Entity> {
        let assets = self.resource::<AssetServer>();
        let patch = SceneListPatch::load(assets, scenes);
        // TODO: return error
        if patch
            .dependencies
            .iter()
            .any(|h| !assets.dependency_load_state(h).is_loaded())
        {
            panic!("This SceneList has unloaded dependencies!");
        }

        let (mut resolved_scenes, entity_scopes) =
            patch.resolve(assets, self.resource::<Assets<ScenePatch>>());
        let mut scoped_entities = ScopedEntities::new(entity_scopes.entity_count());

        let mut entities = Vec::new();
        for scene in resolved_scenes.iter_mut() {
            let mut entity =
                if let Some(scoped_entity_index) = scene.entity_indices.first().copied() {
                    let entity = scoped_entities.get(self, &entity_scopes, scoped_entity_index);
                    self.entity_mut(entity)
                } else {
                    self.spawn_empty()
                };

            entities.push(entity.id());
            scene
                .apply(&mut TemplateContext::new(
                    &mut entity,
                    &mut scoped_entities,
                    &entity_scopes,
                ))
                .unwrap();
        }
        entities
    }
}

pub trait SpawnRelatedScenes {
    fn spawn_related_scenes<T: RelationshipTarget>(self, scenes: impl SceneList) -> Self;
}

impl SpawnRelatedScenes for EntityWorldMut<'_> {
    fn spawn_related_scenes<T: RelationshipTarget>(mut self, scenes: impl SceneList) -> Self {
        let assets = self.resource::<AssetServer>();
        let patch = SceneListPatch::load(assets, scenes);
        let handle = assets.add(patch);
        let entity = self.id();
        self.resource_mut::<NewScenes>()
            .related_scene_list_spawns
            .push((
                RelatedSceneListSpawn {
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
                let (resolved_scenes, entity_scopes) = list_patch.resolve(&assets, &patches);
                list_patch.resolved = Some(resolved_scenes);
                list_patch.entity_scopes = Some(entity_scopes);
            }
            _ => {}
        }
    }
}

#[derive(Resource, Default)]
pub struct QueuedScenes {
    waiting_scene_entities: HashMap<Handle<ScenePatch>, Vec<Entity>>,
    waiting_related_list_entities: HashMap<Handle<SceneListPatch>, Vec<RelatedSceneListSpawn>>,
    waiting_scene_list_spawns: HashMap<Handle<SceneListPatch>, usize>,
}

struct RelatedSceneListSpawn {
    entity: Entity,
    insert: fn(&mut EntityWorldMut, target: Entity),
}

#[derive(Resource, Default)]
pub struct NewScenes {
    entities: Vec<Entity>,
    related_scene_list_spawns: Vec<(RelatedSceneListSpawn, Handle<SceneListPatch>)>,
    scene_list_spawns: Vec<Handle<SceneListPatch>>,
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
                            if new_scenes.entities.is_empty()
                                && new_scenes.related_scene_list_spawns.is_empty()
                                && new_scenes.scene_list_spawns.is_empty()
                            {
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
                                            .waiting_scene_entities
                                            .entry(handle.clone())
                                            .or_default();
                                        entities.push(entity);
                                    }
                                }
                            }

                            let mut new_scenes = world.resource_mut::<NewScenes>();
                            for (scene_list_spawn, handle) in
                                core::mem::take(&mut new_scenes.related_scene_list_spawns)
                            {
                                if let Some((Some(resolved_scenes), Some(entity_scopes))) =
                                    list_patches.get_mut(&handle).map(|p| {
                                        let p = p.into_inner();
                                        (p.resolved.as_mut(), p.entity_scopes.as_ref())
                                    })
                                {
                                    for scene in resolved_scenes {
                                        let mut scoped_entities =
                                            ScopedEntities::new(entity_scopes.entity_count());
                                        let mut child_entity = if let Some(scoped_entity_index) =
                                            scene.entity_indices.first().copied()
                                        {
                                            let entity = scoped_entities.get(
                                                world,
                                                &entity_scopes,
                                                scoped_entity_index,
                                            );
                                            world.entity_mut(entity)
                                        } else {
                                            world.spawn_empty()
                                        };
                                        (scene_list_spawn.insert)(
                                            &mut child_entity,
                                            scene_list_spawn.entity,
                                        );
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut child_entity,
                                                &mut scoped_entities,
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    }
                                } else {
                                    let entities = queued
                                        .waiting_related_list_entities
                                        .entry(handle)
                                        .or_default();
                                    entities.push(scene_list_spawn);
                                }
                            }

                            let mut new_scenes = world.resource_mut::<NewScenes>();
                            for handle in core::mem::take(&mut new_scenes.scene_list_spawns) {
                                if let Some((Some(resolved_scenes), Some(entity_scopes))) =
                                    list_patches.get_mut(&handle).map(|p| {
                                        let p = p.into_inner();
                                        (p.resolved.as_mut(), p.entity_scopes.as_ref())
                                    })
                                {
                                    for scene in resolved_scenes {
                                        let mut scoped_entities =
                                            ScopedEntities::new(entity_scopes.entity_count());
                                        let mut child_entity = if let Some(scoped_entity_index) =
                                            scene.entity_indices.first().copied()
                                        {
                                            let entity = scoped_entities.get(
                                                world,
                                                &entity_scopes,
                                                scoped_entity_index,
                                            );
                                            world.entity_mut(entity)
                                        } else {
                                            world.spawn_empty()
                                        };
                                        scene
                                            .apply(&mut TemplateContext::new(
                                                &mut child_entity,
                                                &mut scoped_entities,
                                                entity_scopes,
                                            ))
                                            .unwrap();
                                    }
                                } else {
                                    let count =
                                        queued.waiting_scene_list_spawns.entry(handle).or_default();
                                    *count += 1;
                                }
                            }
                        }

                        for event in reader.read(&events) {
                            let patches = world.resource::<Assets<ScenePatch>>();
                            if let AssetEvent::LoadedWithDependencies { id } = event
                                && let Some(resolved) =
                                    patches.get(*id).and_then(|p| p.resolved.clone())
                                && let Some(entities) = queued.waiting_scene_entities.remove(id)
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
                            {
                                if let Some(scene_list_spawns) =
                                    queued.waiting_related_list_entities.remove(id)
                                {
                                    for scene_list_spawn in scene_list_spawns {
                                        let mut scoped_entities =
                                            ScopedEntities::new(entity_scopes.entity_count());
                                        for scene in resolved_scenes.iter_mut() {
                                            let mut child_entity =
                                                if let Some(scoped_entity_index) =
                                                    scene.entity_indices.first().copied()
                                                {
                                                    let entity = scoped_entities.get(
                                                        world,
                                                        &entity_scopes,
                                                        scoped_entity_index,
                                                    );
                                                    world.entity_mut(entity)
                                                } else {
                                                    world.spawn_empty()
                                                };
                                            (scene_list_spawn.insert)(
                                                &mut child_entity,
                                                scene_list_spawn.entity,
                                            );
                                            scene
                                                .apply(&mut TemplateContext::new(
                                                    &mut child_entity,
                                                    &mut scoped_entities,
                                                    entity_scopes,
                                                ))
                                                .unwrap();
                                        }
                                    }
                                }

                                if let Some(waiting_list_spawns) =
                                    queued.waiting_scene_list_spawns.remove(id)
                                {
                                    for _ in 0..waiting_list_spawns {
                                        let mut scoped_entities =
                                            ScopedEntities::new(entity_scopes.entity_count());
                                        for scene in resolved_scenes.iter_mut() {
                                            let mut child_entity =
                                                if let Some(scoped_entity_index) =
                                                    scene.entity_indices.first().copied()
                                                {
                                                    let entity = scoped_entities.get(
                                                        world,
                                                        &entity_scopes,
                                                        scoped_entity_index,
                                                    );
                                                    world.entity_mut(entity)
                                                } else {
                                                    world.spawn_empty()
                                                };
                                            // TODO: remove all unwraps like this
                                            scene
                                                .apply(&mut TemplateContext::new(
                                                    &mut child_entity,
                                                    &mut scoped_entities,
                                                    entity_scopes,
                                                ))
                                                .unwrap();
                                        }
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
