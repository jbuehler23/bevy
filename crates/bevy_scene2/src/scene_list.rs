use crate::{ResolveContext, ResolveSceneError, ResolvedScene, Scene};
use bevy_asset::AssetPath;
use variadics_please::all_tuples;

pub trait SceneList: Send + Sync + 'static {
    fn resolve_list(
        &self,
        context: &mut ResolveContext,
        scenes: &mut Vec<ResolvedScene>,
    ) -> Result<(), ResolveSceneError>;

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>);
}

pub struct EntityScene<S>(pub S);

impl<S: Scene> SceneList for EntityScene<S> {
    fn resolve_list(
        &self,
        context: &mut ResolveContext,
        scenes: &mut Vec<ResolvedScene>,
    ) -> Result<(), ResolveSceneError> {
        let mut resolved_scene = ResolvedScene::default();
        self.0.resolve(context, &mut resolved_scene)?;
        scenes.push(resolved_scene);
        Ok(())
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        self.0.register_dependencies(dependencies);
    }
}

macro_rules! scene_list_impl {
    ($($list: ident),*) => {
        impl<$($list: SceneList),*> SceneList for ($($list,)*) {
            fn resolve_list(&self, _context: &mut ResolveContext, _scenes: &mut Vec<ResolvedScene>) -> Result<(), ResolveSceneError> {
                #[allow(
                    non_snake_case,
                    reason = "The names of these variables are provided by the caller, not by us."
                )]
                let ($($list,)*) = self;
                $($list.resolve_list(_context, _scenes)?;)*
                Ok(())
            }

            fn register_dependencies(&self, _dependencies: &mut Vec<AssetPath<'static>>) {
                #[allow(
                    non_snake_case,
                    reason = "The names of these variables are provided by the caller, not by us."
                )]
                let ($($list,)*) = self;
                $($list.register_dependencies(_dependencies);)*
            }
       }
    }
}

all_tuples!(scene_list_impl, 0, 12, P);

impl<S: Scene> SceneList for Vec<S> {
    fn resolve_list(
        &self,
        context: &mut ResolveContext,
        scenes: &mut Vec<ResolvedScene>,
    ) -> Result<(), ResolveSceneError> {
        for scene in self {
            let mut resolved_scene = ResolvedScene::default();
            scene.resolve(context, &mut resolved_scene)?;
            scenes.push(resolved_scene);
        }
        Ok(())
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        for scene in self {
            scene.register_dependencies(dependencies);
        }
    }
}

impl SceneList for Vec<Box<dyn Scene>> {
    fn resolve_list(
        &self,
        context: &mut ResolveContext,
        scenes: &mut Vec<ResolvedScene>,
    ) -> Result<(), ResolveSceneError> {
        for scene in self {
            let mut resolved_scene = ResolvedScene::default();
            scene.resolve(context, &mut resolved_scene)?;
            scenes.push(resolved_scene);
        }
        Ok(())
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        for scene in self {
            scene.register_dependencies(dependencies);
        }
    }
}
