use super::client::Client;
use super::compositor::Compositor;
use super::HarnessError;
use std::collections::HashSet;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_imported_v2::{self, ZxdgImportedV2};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_importer_v2::ZxdgImporterV2;

#[derive(Default)]
struct Imports {
    destroyed: HashSet<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Import(u32);

pub struct Importer {
    client: Client<Imports>,
    importer: ZxdgImporterV2,
    imported: Vec<ZxdgImportedV2>,
}

impl Importer {
    pub fn connect(compositor: &Compositor) -> Result<Self, HarnessError> {
        let client = Client::connect(
            compositor.display(),
            "xdg-foreign importer",
            Imports::default(),
        )?;
        let importer = client.bind(1, ())?;
        Ok(Importer {
            client,
            importer,
            imported: Vec::new(),
        })
    }

    pub fn import(&mut self, handle: &str) -> Result<Import, HarnessError> {
        let import = Import(self.imported.len() as u32);
        let imported =
            self.importer
                .import_toplevel(handle.to_owned(), &self.client.handle(), import.0);
        self.imported.push(imported);
        self.client.roundtrip()?;
        Ok(import)
    }

    pub fn alive(&mut self, import: Import) -> Result<bool, HarnessError> {
        self.client.roundtrip()?;
        Ok(!self.client.state.destroyed.contains(&import.0))
    }
}

impl Dispatch<ZxdgImportedV2, u32> for Imports {
    fn event(
        state: &mut Self,
        _: &ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        import: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_imported_v2::Event::Destroyed = event {
            state.destroyed.insert(*import);
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Imports {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Imports: ZxdgImporterV2);
