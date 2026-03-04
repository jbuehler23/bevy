use crate::{PatchContext, ResolvedScene, Scene, SceneList};
use bevy_asset::{Asset, AssetServer, Assets, Handle, UntypedHandle};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{component::Component, template::EntityScopes};
use bevy_reflect::TypePath;
use std::sync::Arc;

#[derive(Asset, TypePath)]
pub struct ScenePatch {
    pub patch: Box<dyn Scene>,
    #[dependency]
    pub dependencies: Vec<UntypedHandle>,
    // TODO: consider breaking this out to prevent mutating asset events when resolved. Assets as Entities!
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
            patch: Box::new(scene),
            dependencies,
            resolved: None,
        }
    }

    pub fn resolve(
        &self,
        assets: &AssetServer,
        patches: &Assets<ScenePatch>,
    ) -> (ResolvedScene, EntityScopes) {
        let mut scene = ResolvedScene::default();
        let mut entity_scopes = EntityScopes::default();
        self.patch.patch(
            &mut PatchContext {
                assets: &assets,
                patches: &patches,
                current_scope: 0,
                entity_scopes: &mut entity_scopes,
                inherited: None,
            },
            &mut scene,
        );

        (scene, entity_scopes)
    }
}

#[derive(Component, Deref, DerefMut)]
pub struct ScenePatchInstance(pub Handle<ScenePatch>);

#[derive(Asset, TypePath)]
pub struct SceneListPatch {
    pub patch: Box<dyn SceneList>,
    #[dependency]
    pub dependencies: Vec<UntypedHandle>,
    // TODO: consider breaking this out to prevent mutating asset events when resolved
    pub resolved: Option<Vec<ResolvedScene>>,
    pub entity_scopes: Option<EntityScopes>,
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
            patch: Box::new(scene_list),
            dependencies,
            resolved: None,
            entity_scopes: None,
        }
    }
}
