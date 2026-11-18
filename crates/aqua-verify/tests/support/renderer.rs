use aqua_gpu_d3d11::hal::selection::AdapterInfo;
use aqua_verify::budget::MemoryBudget;

pub fn budget_for(adapter: &AdapterInfo, budget: Option<MemoryBudget>) -> Option<MemoryBudget> {
    match adapter.microsoft_renderer() {
        false => budget,
        true => {
            eprintln!(
                "  steady-state private growth check skipped: {} is a Microsoft software renderer, its working memory is process private bytes",
                adapter.name
            );
            budget.map(MemoryBudget::without_steady_growth)
        }
    }
}
