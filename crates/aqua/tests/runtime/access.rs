struct CountingSink {
    applied: Rc<std::cell::Cell<usize>>,
}

impl AccessSink for CountingSink {
    fn needs_snapshot(&self) -> bool {
        true
    }

    fn apply(&mut self, _update: &AccessTreeUpdate) {
        self.applied.set(self.applied.get() + 1);
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        Vec::new()
    }
}

struct Revisioned {
    revision: std::cell::Cell<u64>,
    nodes: std::cell::Cell<usize>,
}

impl Application for Revisioned {
    fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &WindowEvent) {}

    fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &CoreInputEvent) {}

    fn build_scene(&mut self, _window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
        None
    }

    fn a11y_nodes_for(
        &self,
        _id: AquaWindowId,
    ) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<A11yNode>)> {
        self.nodes.set(self.nodes.get() + 1);
        let root = aqua_core::NodeId::from_raw(aqua_core::id::RawId {
            index: 0,
            generation: 0,
        });
        Some((root, root, Vec::new()))
    }

    fn a11y_revision_for(&self, _id: AquaWindowId) -> Option<u64> {
        Some(self.revision.get())
    }
}

#[test]
fn connected_accessibility_skips_snapshots_while_the_revision_is_unchanged() {
    let (mut aqua, _probe, id) = headless();
    let applied = Rc::new(std::cell::Cell::new(0));
    aqua.windows.get_mut(&id).unwrap().access = Some(Box::new(CountingSink {
        applied: Rc::clone(&applied),
    }));
    let app = Revisioned {
        revision: std::cell::Cell::new(1),
        nodes: std::cell::Cell::new(0),
    };
    aqua.sync_access(&app, id);
    aqua.sync_access(&app, id);
    aqua.sync_access(&app, id);
    assert_eq!((applied.get(), app.nodes.get()), (1, 1));
    app.revision.set(2);
    aqua.sync_access(&app, id);
    assert_eq!((applied.get(), app.nodes.get()), (2, 2));
    aqua.windows.get_mut(&id).unwrap().access_epoch = 7;
    aqua.sync_access(&app, id);
    assert_eq!((applied.get(), app.nodes.get()), (3, 3));
}
