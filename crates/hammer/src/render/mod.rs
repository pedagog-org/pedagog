pub mod containerfile;
pub mod describe;

use crate::resolve::plan::{BasePlan, BuildPlan};

pub trait Renderer {
    fn render_build(&self, plan: &BuildPlan) -> String;
    fn render_base(&self, plan: &BasePlan) -> String;
    fn render_build_with_base(&self, base: &BasePlan, build: &BuildPlan) -> String;
}
