//! Configurable key combinations resolve to central dispatcher actions.

use std::collections::HashSet;

use smithay::{
    backend::input::{InputBackend, KeyState, KeyboardKeyEvent},
    input::keyboard::{FilterResult, Keysym, keysyms, xkb},
    utils::SERIAL_COUNTER,
};

use crate::{config::BindSpec, dispatch::Dispatch, state::Villain};

#[derive(Clone, Debug, PartialEq)]
enum KeyboardAction {
    Dispatch(Dispatch),
    Vt(i32),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum BindingKey {
    Keysym(u32),
    Modifier(Modkey),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct KeyCombination {
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
    key: BindingKey,
}

#[derive(Clone, Debug)]
struct Keybind {
    keys: KeyCombination,
    dispatch: Dispatch,
}

#[derive(Clone, Debug)]
pub struct KeybindRegistry {
    bindings: Vec<Keybind>,
}

impl KeybindRegistry {
    pub fn defaults(modkey: &str) -> Result<Self, String> {
        let mut specs = vec![
            spec("MOD", "toggle-overview", &[]),
            spec("MOD+RETURN", "exec", &["kitty"]),
            spec("MOD+Q", "close", &[]),
            spec("MOD+F", "toggle-maximize", &[]),
            spec("MOD+CTRL+LEFT", "resize-master", &["-5"]),
            spec("MOD+CTRL+RIGHT", "resize-master", &["5"]),
            spec("MOD+CTRL+R", "reset-master", &[]),
            spec("MOD+M", "minimize", &[]),
            spec("MOD+SHIFT+M", "restore-minimized", &[]),
            spec("MOD+LEFT", "previous-workspace", &[]),
            spec("MOD+RIGHT", "next-workspace", &[]),
        ];
        for workspace in 1..=9 {
            specs.push(BindSpec {
                keys: format!("MOD+{workspace}"),
                dispatch: "workspace".into(),
                args: vec![workspace.to_string()],
            });
        }
        specs.push(spec("MOD+0", "workspace", &["10"]));
        Self::from_specs(modkey, &specs)
    }

    pub(crate) fn from_specs(modkey: &str, specs: &[BindSpec]) -> Result<Self, String> {
        let modkey = parse_modkey(modkey)?;
        let mut seen = HashSet::new();
        let mut bindings = Vec::with_capacity(specs.len());
        for spec in specs {
            let keys = parse_keys(&spec.keys, modkey)
                .map_err(|error| format!("binding {:?}: {error}", spec.keys))?;
            if !seen.insert(keys) {
                return Err(format!("duplicate keybinding {:?}", spec.keys));
            }
            let dispatch = parse_dispatch(spec)
                .map_err(|error| format!("binding {:?}: {error}", spec.keys))?;
            bindings.push(Keybind { keys, dispatch });
        }
        Ok(Self { bindings })
    }

    fn find(
        &self,
        raw_syms: &[Keysym],
        ctrl: bool,
        alt: bool,
        shift: bool,
        super_key: bool,
    ) -> Option<Dispatch> {
        self.find_binding(raw_syms, ctrl, alt, shift, super_key)
            .map(|binding| binding.dispatch.clone())
    }

    fn find_modifier_only(
        &self,
        raw_syms: &[Keysym],
        ctrl: bool,
        alt: bool,
        shift: bool,
        super_key: bool,
    ) -> Option<Dispatch> {
        self.find_binding(raw_syms, ctrl, alt, shift, super_key)
            .filter(|binding| matches!(binding.keys.key, BindingKey::Modifier(_)))
            .map(|binding| binding.dispatch.clone())
    }

    fn find_binding(
        &self,
        raw_syms: &[Keysym],
        ctrl: bool,
        alt: bool,
        shift: bool,
        super_key: bool,
    ) -> Option<&Keybind> {
        self.bindings.iter().find(|binding| {
            binding.keys.ctrl == ctrl
                && binding.keys.alt == alt
                && binding.keys.shift == shift
                && binding.keys.super_key == super_key
                && binding.keys.key.matches(raw_syms)
        })
    }
}

impl BindingKey {
    fn matches(self, raw_syms: &[Keysym]) -> bool {
        raw_syms.iter().any(|symbol| match self {
            Self::Keysym(expected) => symbol.raw() == expected,
            Self::Modifier(Modkey::Alt) => {
                matches!(symbol.raw(), keysyms::KEY_Alt_L | keysyms::KEY_Alt_R)
            }
            Self::Modifier(Modkey::Super) => {
                matches!(symbol.raw(), keysyms::KEY_Super_L | keysyms::KEY_Super_R)
            }
        })
    }
}

fn spec(keys: &str, dispatch: &str, args: &[&str]) -> BindSpec {
    BindSpec {
        keys: keys.into(),
        dispatch: dispatch.into(),
        args: args.iter().map(|argument| (*argument).into()).collect(),
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Modkey {
    Alt,
    Super,
}

fn parse_modkey(value: &str) -> Result<Modkey, String> {
    match value.to_ascii_lowercase().as_str() {
        "alt" => Ok(Modkey::Alt),
        "super" | "logo" => Ok(Modkey::Super),
        _ => Err(format!("modkey must be Alt or Super, got {value:?}")),
    }
}

fn parse_keys(value: &str, modkey: Modkey) -> Result<KeyCombination, String> {
    let tokens: Vec<_> = value
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_key = false;
    let mut mod_token = false;
    let mut key = None;
    for token in &tokens {
        match token.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => ctrl = true,
            "ALT" => alt = true,
            "SHIFT" => shift = true,
            "SUPER" | "LOGO" => super_key = true,
            "MOD" => match modkey {
                Modkey::Alt => {
                    alt = true;
                    mod_token = true;
                }
                Modkey::Super => {
                    super_key = true;
                    mod_token = true;
                }
            },
            _ if key.is_none() => key = Some(token),
            _ => return Err("a combination must contain exactly one non-modifier key".into()),
        }
    }
    let key = match key {
        Some(key) => {
            let canonical = match key.to_ascii_uppercase().as_str() {
                "ENTER" => "Return",
                "ESC" => "Escape",
                "SPACE" => "space",
                _ => key,
            };
            let sym = xkb::keysym_from_name(canonical, xkb::KEYSYM_CASE_INSENSITIVE).raw();
            if sym == keysyms::KEY_NoSymbol {
                return Err(format!("unknown key name {key:?}"));
            }
            BindingKey::Keysym(sym)
        }
        None if mod_token && tokens.len() == 1 => BindingKey::Modifier(modkey),
        None => return Err("combination has no key".into()),
    };
    Ok(KeyCombination {
        ctrl,
        alt,
        shift,
        super_key,
        key,
    })
}

fn parse_dispatch(spec: &BindSpec) -> Result<Dispatch, String> {
    match spec.dispatch.as_str() {
        "resize-master" if spec.args.len() == 1 => {
            let delta: i16 = spec.args[0]
                .parse()
                .map_err(|_| "resize-master requires an integer percentage change".to_string())?;
            if delta == 0 || !(-80..=80).contains(&delta) {
                return Err("resize-master change must be -80..-1 or 1..80".into());
            }
            Ok(Dispatch::ResizeMaster(delta))
        }
        "resize-master" => Err("resize-master requires exactly one argument".into()),
        "reset-master" if spec.args.is_empty() => Ok(Dispatch::ResetMaster),
        "reset-master" => Err("reset-master does not accept arguments".into()),
        "close" if spec.args.is_empty() => Ok(Dispatch::CloseFocused),
        "maximize" if spec.args.is_empty() => Ok(Dispatch::MaximizeFocused),
        "unmaximize" if spec.args.is_empty() => Ok(Dispatch::UnmaximizeFocused),
        "toggle-maximize" if spec.args.is_empty() => Ok(Dispatch::ToggleMaximizeFocused),
        "minimize" if spec.args.is_empty() => Ok(Dispatch::MinimizeFocused),
        "restore-minimized" if spec.args.is_empty() => Ok(Dispatch::RestoreLastMinimized),
        "previous-workspace" if spec.args.is_empty() => Ok(Dispatch::PreviousWorkspace),
        "next-workspace" if spec.args.is_empty() => Ok(Dispatch::NextWorkspace),
        "quit" if spec.args.is_empty() => Ok(Dispatch::Quit),
        "toggle-overview" if spec.args.is_empty() => Ok(Dispatch::ToggleOverview),
        "exec"
            if spec.keys.eq_ignore_ascii_case("MOD")
                && spec.args == ["knave-shell", "overview"] =>
        {
            Ok(Dispatch::ToggleOverview)
        }
        "exec" if !spec.args.is_empty() => Ok(Dispatch::Spawn(spec.args.clone())),
        "workspace" if spec.args.len() == 1 => {
            let workspace = spec.args[0]
                .parse()
                .map_err(|_| "workspace argument must be a number".to_string())?;
            if !(1..=10).contains(&workspace) {
                return Err("workspace argument must be between 1 and 10".into());
            }
            Ok(Dispatch::FocusWorkspace(workspace))
        }
        "maximize" | "unmaximize" | "toggle-maximize" | "toggle-overview" | "close"
        | "minimize" | "restore-minimized" | "previous-workspace" | "next-workspace" | "quit" => {
            Err("dispatch does not accept arguments".into())
        }
        "exec" => Err("exec requires a program in args".into()),
        "workspace" => Err("workspace requires exactly one argument".into()),
        dispatch => Err(format!("unknown dispatch {dispatch:?}")),
    }
}

pub fn handle_keyboard_event<B: InputBackend>(
    state: &mut Villain,
    event: impl KeyboardKeyEvent<B>,
) {
    let code = event.key_code();
    let pressed = event.state() == KeyState::Pressed;
    let keyboard = state.keyboard.clone();
    let action = keyboard.input(
        state,
        code,
        event.state(),
        SERIAL_COUNTER.next_serial(),
        event.time_msec(),
        |state, mods, key| {
            if !pressed
                && state
                    .pending_modifier
                    .as_ref()
                    .is_some_and(|(pending_code, _)| *pending_code == code)
                && let Some((_, dispatch)) = state.pending_modifier.take()
            {
                state.suppressed_keys.remove(&code);
                return FilterResult::Intercept(Some(KeyboardAction::Dispatch(dispatch)));
            }
            if !pressed && state.suppressed_keys.remove(&code) {
                return FilterResult::Intercept(None);
            }
            if state.suppressed_keys.contains(&code) {
                return FilterResult::Intercept(None);
            }
            if pressed
                && state
                    .pending_modifier
                    .as_ref()
                    .is_some_and(|(pending_code, _)| *pending_code != code)
            {
                state.pending_modifier = None;
            }
            if pressed {
                tracing::debug!(?code, sym = ?key.modified_sym(), "key pressed");
            }
            if pressed && mods.ctrl && mods.alt {
                let sym = key.modified_sym().raw();
                let action = if key
                    .raw_syms()
                    .iter()
                    .any(|sym| sym.raw() == keysyms::KEY_BackSpace)
                {
                    Some(KeyboardAction::Dispatch(Dispatch::Quit))
                } else if state.tty.is_some()
                    && (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&sym)
                {
                    Some(KeyboardAction::Vt(
                        (sym - keysyms::KEY_XF86Switch_VT_1 + 1) as i32,
                    ))
                } else {
                    key.raw_syms().iter().find_map(|sym| {
                        (state.tty.is_some()
                            && (keysyms::KEY_F1..=keysyms::KEY_F12).contains(&sym.raw()))
                        .then(|| KeyboardAction::Vt((sym.raw() - keysyms::KEY_F1 + 1) as i32))
                    })
                };
                if let Some(action) = action {
                    state.suppressed_keys.insert(code);
                    return FilterResult::Intercept(Some(action));
                }
            }
            if pressed
                && let Some(action) = state.config.keybinds.find_modifier_only(
                    &key.raw_syms(),
                    mods.ctrl,
                    mods.alt,
                    mods.shift,
                    mods.logo,
                )
            {
                state.pending_modifier = Some((code, action));
                state.suppressed_keys.insert(code);
                return FilterResult::Intercept(None);
            }
            if pressed
                && let Some(action) = state.config.keybinds.find(
                    &key.raw_syms(),
                    mods.ctrl,
                    mods.alt,
                    mods.shift,
                    mods.logo,
                )
            {
                state.suppressed_keys.insert(code);
                return FilterResult::Intercept(Some(KeyboardAction::Dispatch(action)));
            }
            FilterResult::Forward
        },
    );
    match action.flatten() {
        Some(KeyboardAction::Dispatch(dispatch)) => {
            if let Err(error) = state.dispatch(dispatch) {
                tracing::debug!(%error, "keybind dispatch had no effect");
            }
        }
        Some(KeyboardAction::Vt(vt)) => {
            use smithay::backend::session::Session;
            if let Some(tty) = state.tty.as_mut()
                && let Err(error) = tty.session.change_vt(vt)
            {
                tracing::warn!(%error, "VT switch failed");
            }
        }
        None => {}
    }
    state.flush_pending_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_bindings_validate_delta_and_resolve_modkey() {
        let registry = KeybindRegistry::defaults("Super").unwrap();
        assert_eq!(
            registry.find(&[Keysym::new(keysyms::KEY_Left)], true, false, false, true),
            Some(Dispatch::ResizeMaster(-5))
        );
        assert_eq!(
            registry.find(&[Keysym::new(keysyms::KEY_Right)], true, false, false, true),
            Some(Dispatch::ResizeMaster(5))
        );
        assert_eq!(
            parse_dispatch(&spec("MOD+R", "reset-master", &[])).unwrap(),
            Dispatch::ResetMaster
        );
        assert_eq!(
            parse_dispatch(&spec("MOD+H", "resize-master", &["-2"])).unwrap(),
            Dispatch::ResizeMaster(-2)
        );
        for args in [
            vec![],
            vec!["0"],
            vec!["81"],
            vec!["-81"],
            vec!["NaN"],
            vec!["2", "3"],
        ] {
            assert!(parse_dispatch(&spec("MOD+H", "resize-master", &args)).is_err());
        }
    }

    #[test]
    fn modkey_changes_all_default_bindings() {
        let super_registry = KeybindRegistry::defaults("Super").unwrap();
        let q = [Keysym::new(keysyms::KEY_q)];
        assert_eq!(
            super_registry.find(&q, false, false, false, true),
            Some(Dispatch::CloseFocused)
        );
        assert_eq!(super_registry.find(&q, false, true, false, false), None);

        let alt_registry = KeybindRegistry::defaults("Alt").unwrap();
        assert_eq!(
            alt_registry.find(&q, false, true, false, false),
            Some(Dispatch::CloseFocused)
        );
    }

    #[test]
    fn default_modifier_release_opens_knave_overview() {
        let registry = KeybindRegistry::defaults("Super").unwrap();
        assert_eq!(
            registry.find_modifier_only(
                &[Keysym::new(keysyms::KEY_Super_L)],
                false,
                false,
                false,
                true,
            ),
            Some(Dispatch::ToggleOverview)
        );
    }

    #[test]
    fn legacy_overview_exec_binding_toggles_without_changing_other_execs() {
        assert_eq!(
            parse_dispatch(&spec("MOD", "exec", &["knave-shell", "overview"])).unwrap(),
            Dispatch::ToggleOverview
        );
        assert_eq!(
            parse_dispatch(&spec("MOD", "exec", &["wofi"])).unwrap(),
            Dispatch::Spawn(vec!["wofi".into()])
        );
    }

    #[test]
    fn maximize_binding_and_explicit_actions() {
        let registry = KeybindRegistry::defaults("Super").unwrap();
        assert_eq!(
            registry.find(&[Keysym::new(keysyms::KEY_f)], false, false, false, true),
            Some(Dispatch::ToggleMaximizeFocused)
        );
        for name in ["maximize", "unmaximize", "toggle-maximize"] {
            assert!(KeybindRegistry::from_specs("Super", &[spec("MOD+F", name, &[])]).is_ok());
            assert!(
                KeybindRegistry::from_specs("Super", &[spec("MOD+F", name, &["unexpected"])])
                    .is_err()
            );
        }
    }

    #[test]
    fn invalid_and_duplicate_bindings_are_rejected() {
        assert!(parse_keys("MOD+DOES_NOT_EXIST", Modkey::Super).is_err());
        let duplicate = vec![spec("MOD+Q", "close", &[]), spec("MOD+Q", "minimize", &[])];
        assert!(KeybindRegistry::from_specs("Super", &duplicate).is_err());
    }

    #[test]
    fn configured_exec_keeps_arguments() {
        let registry = KeybindRegistry::from_specs(
            "Super",
            &[spec("MOD+RETURN", "exec", &["kitty", "--single-instance"])],
        )
        .unwrap();
        assert_eq!(
            registry.find(
                &[Keysym::new(keysyms::KEY_Return)],
                false,
                false,
                false,
                true,
            ),
            Some(Dispatch::Spawn(vec![
                "kitty".into(),
                "--single-instance".into()
            ]))
        );
    }

    #[test]
    fn modifier_only_binding_matches_both_sides_of_modkey() {
        let super_registry =
            KeybindRegistry::from_specs("Super", &[spec("MOD", "exec", &["wofi"])]).unwrap();
        assert_eq!(
            super_registry.find(
                &[Keysym::new(keysyms::KEY_Super_L)],
                false,
                false,
                false,
                true,
            ),
            Some(Dispatch::Spawn(vec!["wofi".into()]))
        );
        assert_eq!(
            super_registry.find(
                &[Keysym::new(keysyms::KEY_Super_R)],
                false,
                false,
                false,
                true,
            ),
            Some(Dispatch::Spawn(vec!["wofi".into()]))
        );

        let alt_registry =
            KeybindRegistry::from_specs("Alt", &[spec("MOD", "exec", &["wofi"])]).unwrap();
        assert_eq!(
            alt_registry.find(
                &[Keysym::new(keysyms::KEY_Alt_L)],
                false,
                true,
                false,
                false,
            ),
            Some(Dispatch::Spawn(vec!["wofi".into()]))
        );
        assert_eq!(
            super_registry.find_modifier_only(
                &[Keysym::new(keysyms::KEY_Super_L)],
                false,
                false,
                false,
                true,
            ),
            Some(Dispatch::Spawn(vec!["wofi".into()]))
        );
        assert_eq!(
            super_registry.find_modifier_only(
                &[Keysym::new(keysyms::KEY_a)],
                false,
                false,
                false,
                true,
            ),
            None
        );
    }
}
