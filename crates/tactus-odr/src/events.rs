//! Typed device events.
//!
//! Turns the agent's notifications into an [`Event`] a host can match on.
//! Every variant here was observed on an S88 MK3; see
//! `docs/protocol/mk3-odr-ipc.md` for the captures.

use rmpv::Value;

use crate::{Notification, Registry};

/// A transport button press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportAction {
    /// `daw_play`.
    Play,
    /// `daw_stop`.
    Stop,
    /// `daw_record`.
    Record,
    /// `daw_loop`.
    Loop,
    /// `daw_metro`.
    Metro,
    /// `daw_restart`. Defined by the agent; not yet observed.
    Restart,
    /// `daw_count_in`. Defined by the agent; not yet observed.
    CountIn,
    /// `daw_undo`. Defined by the agent; not yet observed.
    Undo,
    /// `daw_redo`. Defined by the agent; not yet observed.
    Redo,
    /// `daw_quantize`. Defined by the agent; not yet observed.
    Quantize,
    /// `daw_tap_tempo`. Defined by the agent; not yet observed.
    TapTempo,
}

impl TransportAction {
    fn from_symbol(s: &str) -> Option<Self> {
        Some(match s {
            "daw_play" => Self::Play,
            "daw_stop" => Self::Stop,
            "daw_record" => Self::Record,
            "daw_loop" => Self::Loop,
            "daw_metro" => Self::Metro,
            "daw_restart" => Self::Restart,
            "daw_count_in" => Self::CountIn,
            "daw_undo" => Self::Undo,
            "daw_redo" => Self::Redo,
            "daw_quantize" => Self::Quantize,
            "daw_tap_tempo" => Self::TapTempo,
            _ => return None,
        })
    }
}

/// Something the device reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A knob turned by a relative step. The host applies it and echoes the
    /// result with [`crate::Client::set_parameter_value`].
    KnobTurn {
        /// Parameter id.
        id: u32,
        /// Relative step, typically ±0.002–0.02.
        delta: f64,
    },
    /// A finger landed on or lifted from a touch-sensitive knob.
    KnobTouch {
        /// Parameter id.
        id: u32,
        /// `true` on touch, `false` on release.
        touched: bool,
    },
    /// A transport button.
    Transport(TransportAction),
    /// A browser control, such as `browser_navigate_sound` or
    /// `browser_load_next_sound`.
    Browser {
        /// The action symbol.
        action: String,
        /// Its payload: a step for encoders, `nil` for buttons.
        value: Value,
    },
    /// The device changed page.
    Page {
        /// Page symbol, e.g. `parameter`, `browser`.
        page: String,
        /// Whether this instance holds the light guide.
        lightguide_focus: bool,
    },
    /// Who owns the surface now: an instance uuid, or a built-in mode such as
    /// `play_assist` or `standalone_midi`.
    Focus {
        /// The new owner.
        owner: String,
    },
    /// The scale root changed.
    RootKey(i64),
    /// Anything not modelled yet, kept rather than dropped.
    Other {
        /// Notification name.
        name: String,
        /// Its parameters, keys resolved through the registry.
        params: Value,
    },
}

/// Decode one notification.
///
/// Returns one [`Event`] per action. A single `plugin_action` notification can
/// in principle carry several, although every capture so far carried one.
pub fn decode(registry: &Registry, n: &Notification) -> Vec<Event> {
    let name = n.name.clone().unwrap_or_else(|| format!("#{}", n.id));
    let params = registry.humanise(&n.params);
    let items = match &params {
        Value::Array(a) => a.as_slice(),
        _ => &[],
    };
    let other = || {
        vec![Event::Other {
            name: name.clone(),
            params: params.clone(),
        }]
    };

    match name.as_str() {
        // plugin_action(serial, {action: payload}, bool)
        "plugin_action" => {
            let Some(Value::Map(actions)) = items.get(1) else {
                return other();
            };
            actions
                .iter()
                .map(|(k, v)| decode_action(k.as_str().unwrap_or(""), v))
                .collect()
        }
        // device_owned_instance_client_state_changed(serial, {page, has_lightguide_focus})
        "device_owned_instance_client_state_changed" => {
            let Some(Value::Map(fields)) = items.get(1) else {
                return other();
            };
            let get = |key: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k.as_str() == Some(key))
                    .map(|(_, v)| v)
            };
            let page = match get("page") {
                // The page value is itself a symbol.
                Some(v) => v
                    .as_u64()
                    .and_then(|i| u32::try_from(i).ok())
                    .and_then(|i| registry.name(i))
                    .map(str::to_owned)
                    .or_else(|| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                None => String::new(),
            };
            let lightguide_focus = get("has_lightguide_focus")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            vec![Event::Page {
                page,
                lightguide_focus,
            }]
        }
        // focus_changed(serial, owner)
        "focus_changed" => match items.get(1).and_then(Value::as_str) {
            Some(owner) => vec![Event::Focus {
                owner: owner.to_owned(),
            }],
            None => other(),
        },
        // device_properties(serial, {root_key: n})
        "device_properties" => {
            let root = match items.get(1) {
                Some(Value::Map(f)) => f
                    .iter()
                    .find(|(k, _)| k.as_str() == Some("root_key"))
                    .and_then(|(_, v)| v.as_i64()),
                _ => None,
            };
            match root {
                Some(r) => vec![Event::RootKey(r)],
                None => other(),
            }
        }
        _ => other(),
    }
}

fn decode_action(action: &str, payload: &Value) -> Event {
    let pair = || match payload {
        Value::Array(a) => (a.first().cloned(), a.get(1).cloned()),
        _ => (None, None),
    };
    match action {
        "plugin_parameter_value_change" => {
            let (id, delta) = pair();
            match (
                id.and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                delta.and_then(|v| v.as_f64()),
            ) {
                (Some(id), Some(delta)) => Event::KnobTurn { id, delta },
                _ => unknown(action, payload),
            }
        }
        "plugin_parameter_erp_touched" => {
            let (id, on) = pair();
            match (
                id.and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                on.and_then(|v| v.as_bool()),
            ) {
                (Some(id), Some(touched)) => Event::KnobTouch { id, touched },
                _ => unknown(action, payload),
            }
        }
        a if a.starts_with("browser_") => Event::Browser {
            action: a.to_owned(),
            value: payload.clone(),
        },
        a => match TransportAction::from_symbol(a) {
            Some(t) => Event::Transport(t),
            None => unknown(action, payload),
        },
    }
}

fn unknown(action: &str, payload: &Value) -> Event {
    Event::Other {
        name: format!("plugin_action:{action}"),
        params: payload.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    fn s(t: &str) -> Value {
        Value::String(t.into())
    }

    fn note(name: &str, params: Value) -> Notification {
        Notification {
            id: 0,
            name: Some(name.into()),
            params,
        }
    }

    fn action(key: &str, payload: Value) -> Notification {
        note(
            "plugin_action",
            Value::Array(vec![
                s("99FE7169"),
                Value::Map(vec![(s(key), payload)]),
                Value::Boolean(true),
            ]),
        )
    }

    #[test]
    fn a_knob_turn_decodes_with_its_relative_step() {
        // Captured: plugin_action [serial, {plugin_parameter_value_change: [4, -0.002]}, true]
        let r = Registry::default();
        let n = action(
            "plugin_parameter_value_change",
            Value::Array(vec![Value::from(4u32), Value::F64(-0.002)]),
        );
        assert_eq!(
            decode(&r, &n),
            vec![Event::KnobTurn {
                id: 4,
                delta: -0.002
            }]
        );
    }

    #[test]
    fn a_knob_touch_and_release_decode() {
        let r = Registry::default();
        let on = action(
            "plugin_parameter_erp_touched",
            Value::Array(vec![Value::from(4u32), Value::Boolean(true)]),
        );
        let off = action(
            "plugin_parameter_erp_touched",
            Value::Array(vec![Value::from(4u32), Value::Boolean(false)]),
        );
        assert_eq!(
            decode(&r, &on),
            vec![Event::KnobTouch {
                id: 4,
                touched: true
            }]
        );
        assert_eq!(
            decode(&r, &off),
            vec![Event::KnobTouch {
                id: 4,
                touched: false
            }]
        );
    }

    #[test]
    fn every_observed_transport_button_decodes() {
        let r = Registry::default();
        for (sym, want) in [
            ("daw_play", TransportAction::Play),
            ("daw_stop", TransportAction::Stop),
            ("daw_record", TransportAction::Record),
            ("daw_loop", TransportAction::Loop),
            ("daw_metro", TransportAction::Metro),
        ] {
            assert_eq!(
                decode(&r, &action(sym, Value::Nil)),
                vec![Event::Transport(want)]
            );
        }
    }

    #[test]
    fn browser_actions_keep_their_payload() {
        let r = Registry::default();
        let n = action("browser_navigate_sound", Value::from(1u32));
        assert_eq!(
            decode(&r, &n),
            vec![Event::Browser {
                action: "browser_navigate_sound".into(),
                value: Value::from(1u32),
            }]
        );
    }

    #[test]
    fn interned_action_keys_resolve_through_the_registry() {
        // On the wire the action key is a registry id, not a string.
        let mut names = vec![String::new(); 32];
        names[31] = "daw_play".into();
        let r = Registry::from_names(names);
        let n = note(
            "plugin_action",
            Value::Array(vec![
                s("99FE7169"),
                Value::Map(vec![(Value::from(31u32), Value::Nil)]),
                Value::Boolean(true),
            ]),
        );
        assert_eq!(
            decode(&r, &n),
            vec![Event::Transport(TransportAction::Play)]
        );
    }

    #[test]
    fn a_page_change_resolves_the_page_symbol() {
        let mut names = vec![String::new(); 223];
        names[222] = "parameter".into();
        let r = Registry::from_names(names);
        let n = note(
            "device_owned_instance_client_state_changed",
            Value::Array(vec![
                s("99FE7169"),
                Value::Map(vec![
                    (s("page"), Value::from(222u32)),
                    (s("has_lightguide_focus"), Value::Boolean(true)),
                ]),
            ]),
        );
        assert_eq!(
            decode(&r, &n),
            vec![Event::Page {
                page: "parameter".into(),
                lightguide_focus: true,
            }]
        );
    }

    #[test]
    fn focus_moving_to_a_built_in_mode_decodes() {
        let r = Registry::default();
        let n = note(
            "focus_changed",
            Value::Array(vec![s("99FE7169"), s("play_assist")]),
        );
        assert_eq!(
            decode(&r, &n),
            vec![Event::Focus {
                owner: "play_assist".into()
            }]
        );
    }

    #[test]
    fn an_unmodelled_action_is_kept_not_dropped() {
        let r = Registry::default();
        let n = action("smartplay_arp_engine_change", Value::from(3u32));
        assert!(matches!(&decode(&r, &n)[..], [Event::Other { name, .. }]
            if name == "plugin_action:smartplay_arp_engine_change"));
    }
}
