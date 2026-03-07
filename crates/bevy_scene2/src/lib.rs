#![allow(missing_docs)]

pub mod prelude {
    pub use crate::{
        bsn, bsn_list, on, CommandsSpawnScene, LoadScene, PatchGetTemplate, PatchTemplate, Scene,
        SceneList, ScenePatchInstance, SpawnRelatedScenes, SpawnScene,
    };
}

mod resolved_scene;
mod scene;
mod scene_list;
mod scene_patch;
mod spawn;

pub use bevy_scene2_macros::*;

pub use resolved_scene::*;
pub use scene::*;
pub use scene_list::*;
pub use scene_patch::*;
pub use spawn::*;

use bevy_app::{App, Plugin, Update};
use bevy_asset::{AssetApp, AssetPath, AssetServer, Handle};
use bevy_ecs::{
    prelude::*,
    system::IntoObserverSystem,
    template::{Template, TemplateContext},
};
use std::marker::PhantomData;

#[derive(Default)]
pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QueuedScenes>()
            .init_resource::<NewScenes>()
            .init_asset::<ScenePatch>()
            .init_asset::<SceneListPatch>()
            .add_systems(Update, (resolve_scene_patches, spawn_queued).chain())
            .add_observer(on_add_scene_patch_instance);
    }
}

/// This is used by the [`bsn!`] macro to generate compile-time only references to symbols. Currently this is used
/// to add IDE support for nested type names, as it allows us to pass the input Ident from the input to the output code.
pub const fn touch_type<T>() {}

pub trait LoadScene {
    fn load_scene<'a>(
        &self,
        path: impl Into<AssetPath<'a>>,
        scene: impl Scene,
    ) -> Handle<ScenePatch>;
}

impl LoadScene for AssetServer {
    fn load_scene<'a>(
        &self,
        path: impl Into<AssetPath<'a>>,
        scene: impl Scene,
    ) -> Handle<ScenePatch> {
        let scene = ScenePatch::load(self, scene);
        self.load_with_path(path, scene)
    }
}

pub struct OnTemplate<I, E, B, M>(pub I, pub PhantomData<fn() -> (E, B, M)>);

impl<I: IntoObserverSystem<E, B, M> + Clone, E: EntityEvent, B: Bundle, M: 'static> Template
    for OnTemplate<I, E, B, M>
{
    type Output = ();

    fn build(&self, context: &mut TemplateContext) -> Result<Self::Output> {
        context.entity.observe(self.0.clone());
        Ok(())
    }

    fn clone_template(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<
        I: IntoObserverSystem<E, B, M> + Clone + Send + Sync,
        E: EntityEvent,
        B: Bundle,
        M: 'static,
    > Scene for OnTemplate<I, E, B, M>
{
    fn patch(&self, _context: &mut PatchContext, scene: &mut ResolvedScene) {
        scene.push_template(OnTemplate(self.0.clone(), PhantomData));
    }
}

pub fn on<I: IntoObserverSystem<E, B, M>, E: EntityEvent, B: Bundle, M: 'static>(
    observer: I,
) -> OnTemplate<I, E, B, M> {
    OnTemplate(observer, PhantomData)
}

#[macro_export]
#[doc(hidden)]
macro_rules! auto_nest_tuple {
    // direct expansion
    () => { () };
    ($a:expr) => {
        $a
    };
    ($a:expr, $b:expr) => {
        (
            $a,
            $b,
        )
    };
    ($a:expr, $b:expr, $c:expr) => {
        (
            $a,
            $b,
            $c,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
            $h,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr, $i:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
            $h,
            $i,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr, $i:expr, $j:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
            $h,
            $i,
            $j,
        )
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr, $i:expr, $j:expr, $k:expr) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
            $h,
            $i,
            $j,
            $k,
        )
    };

    // recursive expansion
    (
        $a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr,
        $g:expr, $h:expr, $i:expr, $j:expr, $k:expr, $($rest:expr),*
    ) => {
        (
            $a,
            $b,
            $c,
            $d,
            $e,
            $f,
            $g,
            $h,
            $i,
            $j,
            $k,
            $crate::auto_nest_tuple!($($rest),*)
        )
    };
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use crate::{self as bevy_scene2, ScenePlugin};
    use bevy_app::App;
    use bevy_asset::AssetPlugin;
    use bevy_ecs::prelude::*;

    #[derive(Component, GetTemplate)]
    struct Reference(Entity);

    #[test]
    fn constant_values() {
        let mut app = App::new();
        app.add_plugins((AssetPlugin::default(), ScenePlugin::default()));
        let world = app.world_mut();

        const X_AXIS: usize = 1;
        const XAXIS: usize = 2;

        #[derive(Component, GetTemplate)]
        struct Value(usize);

        fn x_axis() -> impl Scene {
            bsn! {Value(X_AXIS)}
        }

        fn xaxis() -> impl Scene {
            bsn! {Value(XAXIS)}
        }

        let entity = world.spawn_scene_immediate(x_axis());
        assert_eq!(entity.get::<Value>().unwrap().0, 1);

        let entity = world.spawn_scene_immediate(xaxis());
        assert_eq!(entity.get::<Value>().unwrap().0, 2);
    }

    #[test]
    fn bsn_name_syntax() {
        let mut app = App::new();
        app.add_plugins((AssetPlugin::default(), ScenePlugin::default()));
        let world = app.world_mut();

        fn a() -> impl Scene {
            bsn! {
                #X
                Children [
                    (:b Reference(#X))
                ]
            }
        }

        fn b() -> impl Scene {
            let inline = bsn! {#Y Reference(#Y) Children [ Reference(#Y)] };
            bsn! {
                #X
                Children [
                    Reference(#X),
                    (inline Reference(#X)),
                ]
            }
        }

        let id = world.spawn_scene_immediate(a()).id();

        let a = world.entity(id);
        let name = a.get::<Name>().unwrap();
        assert_eq!(name.as_str(), "X");

        let children = a.get::<Children>().unwrap();
        assert_eq!(children.len(), 1);

        let b = world.entity(children[0]);
        let reference = b.get::<Reference>().unwrap();
        assert_eq!(reference.0, id);

        let b_name = b.get::<Name>().unwrap();
        assert_eq!(b_name.as_str(), "X");

        let grandchildren = b.get::<Children>().unwrap();
        assert_eq!(grandchildren.len(), 2);

        let grandchild = world.entity(grandchildren[0]);
        assert_eq!(grandchild.get::<Reference>().unwrap().0, b.id());

        let grandchild = world.entity(grandchildren[1]);
        assert_eq!(grandchild.get::<Reference>().unwrap().0, b.id());
        assert_eq!(grandchild.get::<Name>().unwrap().as_str(), "Y");

        assert_eq!(
            grandchild.id(),
            world
                .entity(grandchild.get::<Children>().unwrap()[0])
                .get::<Reference>()
                .unwrap()
                .0
        );
    }

    #[test]
    fn bsn_list_name_syntax() {
        let mut app = App::new();
        app.add_plugins((AssetPlugin::default(), ScenePlugin::default()));
        let world = app.world_mut();

        fn b() -> impl Scene {
            bsn! {
                #Z
                Children [
                    Reference(#Z)
                ]
            }
        }

        fn a() -> impl SceneList {
            bsn_list![
                (
                    #X
                    Reference(#Y)
                    Children [
                        (#Z Reference(#X))
                    ]

                ),
                (
                    #Y
                    Reference(#X)
                    Children [
                        Reference(#Y)
                    ]
                ),
                (:b #Z)
            ]
        }

        let ids = world.spawn_scene_list_immediate(a());
        assert_eq!(ids.len(), 3);

        let e0 = world.entity(ids[0]);
        let name = e0.get::<Name>().unwrap();
        assert_eq!(name.as_str(), "X");
        let reference = e0.get::<Reference>().unwrap();
        assert_eq!(reference.0, ids[1]);

        let child0 = e0.get::<Children>().unwrap()[0];
        let reference = world.entity(child0).get::<Reference>().unwrap();
        assert_eq!(reference.0, ids[0]);

        let e1 = world.entity(ids[1]);
        let name = e1.get::<Name>().unwrap();
        assert_eq!(name.as_str(), "Y");

        let reference = e1.get::<Reference>().unwrap();
        assert_eq!(reference.0, ids[0]);

        let child0 = e1.get::<Children>().unwrap()[0];
        let reference = world.entity(child0).get::<Reference>().unwrap();
        assert_eq!(reference.0, ids[1]);

        let e2 = world.entity(ids[2]);
        let name = e2.get::<Name>().unwrap();
        assert_eq!(name.as_str(), "Z");
        let child0 = e2.get::<Children>().unwrap()[0];
        let reference = world.entity(child0).get::<Reference>().unwrap();
        assert_eq!(reference.0, ids[2]);
    }
}
