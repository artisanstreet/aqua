use aqua_core::Oklch;
use aqua_graph::{
    CustomPassSpec, Extent, FramePlan, PaintCommand, PassKind, PlanLimits, ToneCurve,
};

const MAX_COMMANDS: usize = 4096;
const MAX_INSTANCES: usize = 32_768;
const MAX_SHADER_BYTES: usize = 1 << 20;

struct Entry {
    commands: Vec<PaintCommand>,
    plan: FramePlan,
    limits: PlanLimits,
}

#[derive(Default)]
pub(crate) struct RetainedPlan {
    entry: Option<Entry>,
}

fn same_program(a: &CustomPassSpec, b: &CustomPassSpec) -> bool {
    a.reference == b.reference
        && a.name == b.name
        && a.entry == b.entry
        && a.hlsl == b.hlsl
        && a.bounds == b.bounds
        && a.params.len() == b.params.len()
}

fn uniform_update(before: &[PaintCommand], after: &[PaintCommand]) -> Option<Option<usize>> {
    if before.len() != after.len() {
        return None;
    }
    let mut changed = None;
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        if before == after {
            continue;
        }
        match (before, after) {
            (
                PaintCommand::PushCustomEffect {
                    bounds: a,
                    spec: before,
                },
                PaintCommand::PushCustomEffect {
                    bounds: b,
                    spec: after,
                },
            ) if a == b && same_program(before, after) && changed.is_none() => {
                changed = Some(index);
            }
            _ => return None,
        }
    }
    Some(changed)
}

fn shader_bytes(commands: &[PaintCommand]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            PaintCommand::PushCustomEffect { spec, .. } => {
                spec.name.capacity()
                    + spec.entry.capacity()
                    + spec.hlsl.capacity()
                    + spec.params.capacity() * size_of::<f32>()
            }
            PaintCommand::Linear { leaf, .. } => shader_bytes(std::slice::from_ref(leaf)),
            _ => 0,
        })
        .sum()
}

fn source_instances(commands: &[PaintCommand]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            PaintCommand::Glyphs(run) => run.glyphs.len(),
            PaintCommand::Slug(run) => run.glyphs.len(),
            PaintCommand::Linear { leaf, .. } => source_instances(std::slice::from_ref(leaf)),
            _ => 1,
        })
        .sum()
}

impl RetainedPlan {
    pub fn get(&self) -> Option<&FramePlan> {
        self.entry.as_ref().map(|entry| &entry.plan)
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }

    pub fn reuse(
        &mut self,
        extent: Extent,
        clear: Oklch,
        tone: ToneCurve,
        limits: PlanLimits,
        commands: &[PaintCommand],
    ) -> Option<&FramePlan> {
        let entry = self.entry.as_mut()?;
        if entry.plan.extent != extent
            || entry.plan.clear != clear
            || entry.plan.text_tone != tone
            || entry.limits != limits
        {
            return None;
        }
        if let Some(index) = uniform_update(&entry.commands, commands)? {
            let PaintCommand::PushCustomEffect { spec, .. } = &commands[index] else {
                return None;
            };
            if entry
                .commands
                .iter()
                .filter(|command| matches!(command, PaintCommand::PushCustomEffect { .. }))
                .count()
                != 1
            {
                return None;
            }
            let mut passes = entry
                .plan
                .passes
                .iter_mut()
                .filter_map(|pass| match &mut pass.kind {
                    PassKind::Custom(spec) => Some(spec),
                    _ => None,
                });
            let planned = passes.next()?;
            if passes.next().is_some()
                || planned.name != spec.name
                || planned.entry != spec.entry
                || planned.hlsl != spec.hlsl
                || planned.reference != spec.reference
            {
                return None;
            }
            planned.params.clone_from(&spec.params);
            let PaintCommand::PushCustomEffect { spec: previous, .. } = &mut entry.commands[index]
            else {
                return None;
            };
            previous.params.clone_from(&spec.params);
        }
        Some(&entry.plan)
    }

    pub fn store(
        &mut self,
        commands: &[PaintCommand],
        plan: FramePlan,
        limits: PlanLimits,
    ) -> &FramePlan {
        self.entry = Some(Entry {
            commands: commands.to_vec(),
            plan,
            limits,
        });
        &self
            .entry
            .as_ref()
            .expect("the retained plan was just stored")
            .plan
    }

    pub fn bounded(&self) -> bool {
        self.entry.as_ref().is_none_or(|entry| {
            entry.commands.len() <= MAX_COMMANDS
                && entry.plan.instance_count() <= MAX_INSTANCES
                && source_instances(&entry.commands) <= MAX_INSTANCES
                && shader_bytes(&entry.commands) <= MAX_SHADER_BYTES
        })
    }
}
