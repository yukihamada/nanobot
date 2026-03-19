use leptos::prelude::*;

use crate::components::chat::chat_view::ChatView;
use crate::components::layout::header::Header;
use crate::components::layout::sidebar::Sidebar;

#[component]
pub fn HomePage() -> impl IntoView {
    let show_sidebar = RwSignal::new(false);

    let toggle_sidebar = Callback::new(move |_: ()| {
        show_sidebar.update(|v| *v = !*v);
    });

    view! {
        <div class="flex h-full">
            <Sidebar show=show_sidebar />
            <div class="flex-1 flex flex-col min-w-0">
                <Header on_toggle_sidebar=toggle_sidebar />
                <ChatView />
            </div>
        </div>
    }
}
