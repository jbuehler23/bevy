use crate::{ApplySceneError, ResolveContext, ResolveSceneError, ResolvedScene, Scene, SceneList};
use bevy_asset::{Asset, AssetServer, Assets, Handle, UntypedHandle};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    template::{EntityScopes, ScopedEntities, TemplateContext},
    world::{EntityWorldMut, World},
};
use bevy_reflect::TypePath;
use std::sync::Arc;
use thiserror::Error;

#[derive(Asset, TypePath)]
pub struct ScenePatch {
    pub scene: Box<dyn Scene>,
    #[dependency]
    pub dependencies: Vec<UntypedHandle>,
    // TODO: consider breaking this out to prevent mutating asset events when resolved. Assets as Entities will enable this!
    // TODO: This Arc exists to allow nested ResolvedScene::apply when borrowing inherited ScenePatch assets (see the ResolvedScene::apply implementation).
    pub resolved: Option<Arc<(ResolvedScene, EntityScopes)>>,
}

impl ScenePatch {
    pub fn load<P: Scene>(assets: &AssetServer, scene: P) -> Self {
        let mut dependencies = Vec::new();
        scene.register_dependencies(&mut dependencies);
        let dependencies = dependencies
            .iter()
            .map(|i| assets.load::<ScenePatch>(i.clone()).untyped())
            .collect::<Vec<_>>();
        ScenePatch {
            scene: Box::new(scene),
            dependencies,
            resolved: None,
        }
    }

    pub fn resolve(
        &self,
        assets: &AssetServer,
        patches: &Assets<ScenePatch>,
    ) -> Result<(ResolvedScene, EntityScopes), ResolveSceneError> {
        let mut scene = ResolvedScene::default();
        let mut entity_scopes = EntityScopes::default();
        self.scene.resolve(
            &mut ResolveContext {
                assets: &assets,
                patches: &patches,
                current_scope: 0,
                entity_scopes: &mut entity_scopes,
                inherited: None,
            },
            &mut scene,
        )?;

        Ok((scene, entity_scopes))
    }

    pub fn spawn_or_apply<'w>(
        &self,
        world: &'w mut World,
    ) -> Result<EntityWorldMut<'w>, SpawnSceneError> {
        let (resolved, entity_scopes) = self
            .resolved
            .as_ref()
            .map(|r| &**r)
            .ok_or(SpawnSceneError::UnresolvedSceneError)?;
        let mut scoped_entities = ScopedEntities::new(entity_scopes.entity_len());
        resolved
            .spawn_or_apply(world, entity_scopes, &mut scoped_entities)
            .map_err(|e| SpawnSceneError::ApplySceneError(e))
    }
}

#[derive(Error, Debug)]
pub enum SpawnSceneError {
    #[error(transparent)]
    ApplySceneError(#[from] ApplySceneError),
    #[error(transparent)]
    ResolveSceneError(#[from] ResolveSceneError),
    #[error("This scene has not been resolved yet and cannot be spawned. It is likely waiting for dependencies to load")]
    UnresolvedSceneError,
}

#[derive(Component, Deref, DerefMut)]
pub struct ScenePatchInstance(pub Handle<ScenePatch>);

#[derive(Asset, TypePath)]
pub struct SceneListPatch {
    pub scene_list: Box<dyn SceneList>,
    #[dependency]
    pub dependencies: Vec<UntypedHandle>,
    // TODO: consider breaking this out to prevent mutating asset events when resolved
    pub resolved: Option<(Vec<ResolvedScene>, EntityScopes)>,
}

impl SceneListPatch {
    pub fn load<L: SceneList>(assets: &AssetServer, scene_list: L) -> Self {
        let mut dependencies = Vec::new();
        scene_list.register_dependencies(&mut dependencies);
        let dependencies = dependencies
            .iter()
            .map(|i| assets.load::<ScenePatch>(i.clone()).untyped())
            .collect::<Vec<_>>();
        SceneListPatch {
            scene_list: Box::new(scene_list),
            dependencies,
            resolved: None,
        }
    }

    pub fn resolve(
        &self,
        assets: &AssetServer,
        patches: &Assets<ScenePatch>,
    ) -> Result<(Vec<ResolvedScene>, EntityScopes), ResolveSceneError> {
        let mut scenes = Vec::new();
        let mut entity_scopes = EntityScopes::default();
        self.scene_list.resolve_list(
            &mut ResolveContext {
                assets: &assets,
                patches: &patches,
                current_scope: 0,
                entity_scopes: &mut entity_scopes,
                inherited: None,
            },
            &mut scenes,
        )?;

        Ok((scenes, entity_scopes))
    }

    pub fn spawn_or_apply(&self, world: &mut World) -> Result<Vec<Entity>, SpawnSceneError> {
        self.spawn_or_apply_with(world, |_| {})
    }

    pub(crate) fn spawn_or_apply_with(
        &self,
        world: &mut World,
        func: impl Fn(&mut EntityWorldMut),
    ) -> Result<Vec<Entity>, SpawnSceneError> {
        let (resolved_scenes, entity_scopes) = self
            .resolved
            .as_ref()
            .ok_or(SpawnSceneError::UnresolvedSceneError)?;
        let mut scoped_entities = ScopedEntities::new(entity_scopes.entity_len());

        let mut entities = Vec::new();
        for scene in resolved_scenes.iter() {
            let mut entity =
                if let Some(scoped_entity_index) = scene.entity_indices.first().copied() {
                    let entity = scoped_entities.get(world, &entity_scopes, scoped_entity_index);
                    world.entity_mut(entity)
                } else {
                    world.spawn_empty()
                };
            func(&mut entity);

            entities.push(entity.id());
            scene
                .apply(&mut TemplateContext::new(
                    &mut entity,
                    &mut scoped_entities,
                    &entity_scopes,
                ))
                .unwrap();
        }

        Ok(entities)
    }
}
