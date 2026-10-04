use ::hudhook::hooks::dx11::ImguiDx11Hooks;
use ::hudhook::imgui::*;
use ::hudhook::windows::Win32::Foundation::HINSTANCE;
use ::hudhook::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F9, VK_SHIFT};
use ::hudhook::*;
use anyhow::{Result, anyhow};

/// Hidden FPS counter, toggled with Shift+F9.
#[derive(Default)]
struct FpsCounter {
    visible: bool,
    f9_was_down: bool,
}

fn key_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16) & 0x8000 != 0
}

impl ImguiRenderLoop for FpsCounter {
    fn render(&mut self, ui: &mut Ui) {
        let f9_down = key_down(VK_F9.0);
        if f9_down && !self.f9_was_down && key_down(VK_SHIFT.0) {
            self.visible = !self.visible;
        }
        self.f9_was_down = f9_down;

        if !self.visible {
            return;
        }
        ui.window("##fps")
            .position([4., 4.], Condition::Always)
            .bg_alpha(0.35)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_INPUTS
                    | WindowFlags::NO_NAV
                    | WindowFlags::ALWAYS_AUTO_RESIZE
                    | WindowFlags::NO_SAVED_SETTINGS
                    | WindowFlags::NO_FOCUS_ON_APPEARING,
            )
            .build(|| ui.text(format!("{:.0}", ui.io().framerate)));
    }
}

pub fn init(hmodule: HINSTANCE) -> Result<()> {
    Hudhook::builder()
        .with::<ImguiDx11Hooks>(FpsCounter::default())
        .with_hmodule(hmodule)
        .build()
        .apply()
        .map_err(|e| anyhow!("couldn't apply hooks: {e:?}"))
}
