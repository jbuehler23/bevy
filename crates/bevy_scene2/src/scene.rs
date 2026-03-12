use crate::{ResolvedRelatedScenes, ResolvedScene, SceneList, ScenePatch};
use bevy_asset::{AssetPath, AssetServer, Assets};
use bevy_ecs::{
    bundle::Bundle,
    error::Result,
    name::Name,
    relationship::Relationship,
    template::{
        EntityScopes, FnTemplate, GetTemplate, ScopedEntityIndex, Template, TemplateContext,
    },
};
use std::{any::TypeId, marker::PhantomData};
use thiserror::Error;
use variadics_please::all_tuples;

pub trait Scene: Send + Sync + 'static {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError>;
    fn register_dependencies(&self, _dependencies: &mut Vec<AssetPath<'static>>) {}
}

#[derive(Error, Debug)]
pub enum ResolveSceneError {
    #[error("Cannot resolve this inherited scene because it does not exist ({0}). This could be because it isn't loaded yet, or because the asset does not exist. Consider using `queue_spawn_scene()` if you would like to wait for scene dependencies before spawning.")]
    MissingInheritedScene(AssetPath<'static>),
    #[error("Attempted to inherit from a second scene ({0}), which is not allowed.")]
    MultipleInheritance(AssetPath<'static>),
}

pub struct ResolveContext<'a> {
    pub assets: &'a AssetServer,
    pub patches: &'a Assets<ScenePatch>,
    pub inherited: Option<&'a ScenePatch>,
    pub(crate) entity_scopes: &'a mut EntityScopes,
    pub(crate) current_scope: usize,
}

impl<'a> ResolveContext<'a> {
    #[inline]
    pub fn current_scope(&self) -> usize {
        self.current_scope
    }

    pub fn new_scope<T>(&mut self, func: impl FnOnce(&mut ResolveContext) -> T) -> T {
        let current_scope = self.entity_scopes.add_scope();
        let mut context = ResolveContext {
            assets: self.assets,
            patches: self.patches,
            inherited: None,
            entity_scopes: self.entity_scopes,
            current_scope,
        };
        (func)(&mut context)
    }
}

macro_rules! scene_impl {
    ($($patch: ident),*) => {
        impl<$($patch: Scene),*> Scene for ($($patch,)*) {
            fn resolve(&self, _context: &mut ResolveContext, _scene: &mut ResolvedScene) -> Result<(), ResolveSceneError> {
                #[allow(
                    non_snake_case,
                    reason = "The names of these variables are provided by the caller, not by us."
                )]
                let ($($patch,)*) = self;
                $($patch.resolve(_context, _scene)?;)*
                Ok(())
            }

            fn register_dependencies(&self, _dependencies: &mut Vec<AssetPath<'static>>) {
                #[allow(
                    non_snake_case,
                    reason = "The names of these variables are provided by the caller, not by us."
                )]
                let ($($patch,)*) = self;
                $($patch.register_dependencies(_dependencies);)*
            }
       }
    }
}

all_tuples!(scene_impl, 0, 12, P);

pub struct TemplatePatch<F: Fn(&mut T, &mut ResolveContext), T>(pub F, pub PhantomData<T>);

pub fn template_value<T: Template + Default + Clone>(
    value: T,
) -> TemplatePatch<impl Fn(&mut T, &mut ResolveContext), T> {
    TemplatePatch(
        move |input: &mut T, _context: &mut ResolveContext| {
            *input = value.clone();
        },
        PhantomData,
    )
}

pub trait PatchGetTemplate {
    type Template;
    fn patch<F: Fn(&mut Self::Template, &mut ResolveContext)>(
        func: F,
    ) -> TemplatePatch<F, Self::Template>;
}

impl<G: GetTemplate> PatchGetTemplate for G {
    type Template = G::Template;
    fn patch<F: Fn(&mut Self::Template, &mut ResolveContext)>(
        func: F,
    ) -> TemplatePatch<F, Self::Template> {
        TemplatePatch(func, PhantomData)
    }
}

pub trait PatchTemplate: Sized {
    fn patch_template<F: Fn(&mut Self, &mut ResolveContext)>(func: F) -> TemplatePatch<F, Self>;
}

impl<T: Template> PatchTemplate for T {
    fn patch_template<F: Fn(&mut Self, &mut ResolveContext)>(func: F) -> TemplatePatch<F, Self> {
        TemplatePatch(func, PhantomData)
    }
}

impl<
        F: Fn(&mut T, &mut ResolveContext) + Send + Sync + 'static,
        T: Template<Output: Bundle> + Send + Sync + Default + 'static,
    > Scene for TemplatePatch<F, T>
{
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        let template = scene.get_or_insert_template::<T>(context);
        (self.0)(template, context);
        Ok(())
    }
}

pub struct RelatedScenes<R: Relationship, L: SceneList> {
    pub related_template_list: L,
    pub marker: PhantomData<R>,
}

impl<R: Relationship, L: SceneList> RelatedScenes<R, L> {
    pub fn new(list: L) -> Self {
        Self {
            related_template_list: list,
            marker: PhantomData,
        }
    }
}

impl<R: Relationship, L: SceneList> Scene for RelatedScenes<R, L> {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        let related = scene
            .related
            .entry(TypeId::of::<R>())
            .or_insert_with(ResolvedRelatedScenes::new::<R>);
        self.related_template_list
            .resolve_list(context, &mut related.scenes)
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        self.related_template_list
            .register_dependencies(dependencies);
    }
}

pub struct InheritScene<S: Scene>(pub S);

impl<S: Scene> Scene for InheritScene<S> {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        context.new_scope(|context| self.0.resolve(context, scene))
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        self.0.register_dependencies(dependencies);
    }
}

#[derive(Clone)]
pub struct InheritSceneAsset(pub AssetPath<'static>);

impl<I: Into<AssetPath<'static>>> From<I> for InheritSceneAsset {
    fn from(value: I) -> Self {
        InheritSceneAsset(value.into())
    }
}

impl Scene for InheritSceneAsset {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        if let Some(handle) = context.assets.get_handle::<ScenePatch>(&self.0)
            && let Some(scene_patch) = context.patches.get(&handle)
        {
            if context.inherited.is_some() {
                return Err(ResolveSceneError::MultipleInheritance(self.0.clone()));
            }
            context.inherited = Some(scene_patch);
            scene.inherited = Some(handle);
            Ok(())
        } else {
            Err(ResolveSceneError::MissingInheritedScene(self.0.clone()))
        }
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        dependencies.push(self.0.clone())
    }
}

impl<F: (Fn(&mut TemplateContext) -> Result<O>) + Clone + Send + Sync + 'static, O: Bundle> Scene
    for FnTemplate<F, O>
{
    fn resolve(
        &self,
        _context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        scene.push_template(FnTemplate(self.0.clone()));
        Ok(())
    }
}

pub struct NameEntityReference {
    pub name: Name,
    pub index: usize,
}

impl Scene for NameEntityReference {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        let this_index = ScopedEntityIndex {
            scope: context.current_scope(),
            index: self.index,
        };
        if let Some(first_index) = scene.entity_indices.first().copied() {
            let consolidated_index = context.entity_scopes.get(first_index).unwrap();
            context.entity_scopes.assign(this_index, consolidated_index);
        } else {
            context.entity_scopes.alloc(this_index);
        }
        scene.entity_indices.push(this_index);
        let name = scene.get_or_insert_template::<Name>(context);
        *name = self.name.clone();
        Ok(())
    }
}

pub struct SceneScope<S: Scene>(pub S);

impl<S: Scene> Scene for SceneScope<S> {
    fn resolve(
        &self,
        context: &mut ResolveContext,
        scene: &mut ResolvedScene,
    ) -> Result<(), ResolveSceneError> {
        context.new_scope(|context| self.0.resolve(context, scene))
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        self.0.register_dependencies(dependencies);
    }
}

pub struct SceneListScope<L: SceneList>(pub L);

impl<L: SceneList> SceneList for SceneListScope<L> {
    fn resolve_list(
        &self,
        context: &mut ResolveContext,
        scenes: &mut Vec<ResolvedScene>,
    ) -> Result<(), ResolveSceneError> {
        context.new_scope(|context| self.0.resolve_list(context, scenes))
    }

    fn register_dependencies(&self, dependencies: &mut Vec<AssetPath<'static>>) {
        self.0.register_dependencies(dependencies);
    }
}
