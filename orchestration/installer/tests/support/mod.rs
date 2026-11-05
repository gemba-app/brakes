//! Shared by the test files that build module lists with `setup-core` in them
//! (product-boundary T7b, #1057): with the feature they get the real module;
//! without it a stand-in with the same id that renders nothing, so every other
//! module's tests run in both builds. Not every file uses every item.
#![allow(dead_code, unused_imports)]

/// `setup-core` is not compiled in: a module with its id and no content.
pub struct SetupCore;

impl brakes::engine::Module for SetupCore {
    fn id(&self) -> &'static str {
        "setup-core"
    }
    fn version(&self) -> &'static str {
        brakes::modules::core_version()
    }
    fn render(
        &self,
        _: &brakes::engine::Context<'_>,
    ) -> brakes::Result<Vec<brakes::engine::Rendered>> {
        Ok(Vec::new())
    }
}

/// The guides a build installs, by file name: none, the product ships no engineering guide.
pub fn guides() -> &'static [(&'static str, &'static str)] {
    &[]
}
