//! Every button in the app: the kit button with a pointer cursor and a comfortable minimum hit area.
use gpui_kit::component::button::Button as KitButton;
use gpui_kit::*;

pub struct Button;

impl Button {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(id: impl Into<ElementId>) -> KitButton {
        KitButton::new(id).cursor_pointer().min_h(px(28.)).min_w(px(28.))
    }
}
