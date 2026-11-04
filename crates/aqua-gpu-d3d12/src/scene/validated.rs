use aqua_graph::{validate, FramePlan, PlanError};

#[derive(Default)]
pub struct Validated {
    last: Option<FramePlan>,
}

impl Validated {
    pub fn check(&mut self, plan: &FramePlan) -> Result<(), PlanError> {
        if self.last.as_ref() == Some(plan) {
            return Ok(());
        }
        self.last = None;
        validate(plan)?;
        self.last = Some(plan.clone());
        Ok(())
    }
}
