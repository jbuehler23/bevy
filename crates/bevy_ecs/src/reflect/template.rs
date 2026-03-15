//! Definitions for `GetTemplate` and `Template` reflection.

use alloc::boxed::Box;
use core::any::TypeId;

use bevy_reflect::{FromType, Reflect};
use derive_more::{Deref, DerefMut};

use crate::{
    error::BevyError,
    prelude::{GetTemplate, Template},
    template::TemplateContext,
};

#[derive(Clone, Deref, DerefMut)]
pub struct ReflectGetTemplate(pub ReflectGetTemplateData);

#[derive(Clone, Deref, DerefMut)]
pub struct ReflectTemplate(pub ReflectTemplateData);

#[derive(Clone)]
pub struct ReflectGetTemplateData {
    pub template_type_id: TypeId,
}

#[derive(Clone)]
pub struct ReflectTemplateData {
    pub build_template:
        fn(&dyn Reflect, &mut TemplateContext) -> Result<Box<dyn Reflect>, BevyError>,
}

impl<T> FromType<T> for ReflectGetTemplate
where
    T: GetTemplate,
    T::Template: 'static,
    <T::Template as Template>::Output: Reflect,
{
    fn from_type() -> Self {
        ReflectGetTemplate(ReflectGetTemplateData {
            template_type_id: TypeId::of::<T::Template>(),
        })
    }
}

impl<T> FromType<T> for ReflectTemplate
where
    T: Template + 'static,
    <T as Template>::Output: Reflect,
{
    fn from_type() -> Self {
        ReflectTemplate(ReflectTemplateData {
            build_template: |this, context| {
                let Some(this) = this.downcast_ref::<T>() else {
                    return Err("Unexpected `build_template` receiver type".into());
                };
                Ok(Box::new(<T as Template>::build_template(this, context)?))
            },
        })
    }
}
