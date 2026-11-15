//! Allocation-free decision kernel shared by the policy crate and the engine.
//!
//! The allow/deny ordering and the approval-window comparison live here, and
//! only here, so formal verification covers the one copy callers use.

/// The ten boolean facts a decision is derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub action_known: bool,
    pub agent_known: bool,
    pub sources_known: bool,
    pub has_sources: bool,
    pub agent_listed: bool,
    pub agent_may_use_inputs: bool,
    pub recipients_in_audience: bool,
    pub audience_in_readers: bool,
    pub approval_required: bool,
    pub approval_live: bool,
}

/// The outcome of the decision order. Variants are listed in evaluation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    UnknownAction,
    UnknownAgent,
    UnknownSource,
    EmptyOutput,
    AgentNotListed,
    AgentCannotUseSource,
    RecipientsOutsideAudience,
    AudienceOutsideReaders,
    NeedsApproval,
    Allowed,
}

/// Returns the first failing fact in the enum's order; approval is considered
/// only once all eight permission facts hold.
pub const fn gate(facts: Facts) -> Gate {
    if !facts.action_known {
        Gate::UnknownAction
    } else if !facts.agent_known {
        Gate::UnknownAgent
    } else if !facts.sources_known {
        Gate::UnknownSource
    } else if !facts.has_sources {
        Gate::EmptyOutput
    } else if !facts.agent_listed {
        Gate::AgentNotListed
    } else if !facts.agent_may_use_inputs {
        Gate::AgentCannotUseSource
    } else if !facts.recipients_in_audience {
        Gate::RecipientsOutsideAudience
    } else if !facts.audience_in_readers {
        Gate::AudienceOutsideReaders
    } else if facts.approval_required && !facts.approval_live {
        Gate::NeedsApproval
    } else {
        Gate::Allowed
    }
}

impl Gate {
    /// True for the eight permission gates; false for `NeedsApproval` and `Allowed`.
    pub const fn is_denial(self) -> bool {
        !matches!(self, Gate::NeedsApproval | Gate::Allowed)
    }

    /// Stable snake_case code for diagnostics and audit records.
    pub const fn code(self) -> &'static str {
        match self {
            Gate::UnknownAction => "unknown_action",
            Gate::UnknownAgent => "unknown_agent",
            Gate::UnknownSource => "unknown_source",
            Gate::EmptyOutput => "empty_output",
            Gate::AgentNotListed => "agent_not_listed_for_action",
            Gate::AgentCannotUseSource => "agent_cannot_use_source",
            Gate::RecipientsOutsideAudience => "recipients_outside_audience",
            Gate::AudienceOutsideReaders => "audience_outside_readers",
            Gate::NeedsApproval => "needs_approval",
            Gate::Allowed => "allowed",
        }
    }
}

/// True exactly when `issued_at <= now < issued_at + valid_for`, with no
/// overflow, and the approval is not voided.
pub const fn approval_live(issued_at: u64, valid_for: u64, now: u64, voided: bool) -> bool {
    !voided && issued_at <= now && now - issued_at < valid_for
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_from(bits: u16) -> Facts {
        let bit = |n: u16| bits & (1 << n) != 0;
        Facts {
            action_known: bit(0),
            agent_known: bit(1),
            sources_known: bit(2),
            has_sources: bit(3),
            agent_listed: bit(4),
            agent_may_use_inputs: bit(5),
            recipients_in_audience: bit(6),
            audience_in_readers: bit(7),
            approval_required: bit(8),
            approval_live: bit(9),
        }
    }

    fn all_facts() -> impl Iterator<Item = Facts> {
        (0..1024u16).map(facts_from)
    }

    fn permitted(f: &Facts) -> bool {
        f.action_known
            && f.agent_known
            && f.sources_known
            && f.has_sources
            && f.agent_listed
            && f.agent_may_use_inputs
            && f.recipients_in_audience
            && f.audience_in_readers
    }

    #[test]
    fn allowed_requires_every_fact() {
        for f in all_facts() {
            let expected = permitted(&f) && (!f.approval_required || f.approval_live);
            assert_eq!(gate(f) == Gate::Allowed, expected, "{f:?}");
        }
    }

    #[test]
    fn approval_never_overrides_a_denial() {
        for f in all_facts() {
            let g = gate(f);
            if !g.is_denial() {
                continue;
            }
            for (required, live) in [(false, false), (false, true), (true, false), (true, true)] {
                let varied = Facts {
                    approval_required: required,
                    approval_live: live,
                    ..f
                };
                assert_eq!(gate(varied), g, "{varied:?}");
            }
        }
    }

    #[test]
    fn needs_approval_only_when_otherwise_allowed() {
        for f in all_facts() {
            let expected = permitted(&f) && f.approval_required && !f.approval_live;
            assert_eq!(gate(f) == Gate::NeedsApproval, expected, "{f:?}");
        }
    }

    #[test]
    fn first_failing_fact_wins() {
        let ordered = [
            Gate::UnknownAction,
            Gate::UnknownAgent,
            Gate::UnknownSource,
            Gate::EmptyOutput,
            Gate::AgentNotListed,
            Gate::AgentCannotUseSource,
            Gate::RecipientsOutsideAudience,
            Gate::AudienceOutsideReaders,
        ];
        for f in all_facts() {
            let flags = [
                f.action_known,
                f.agent_known,
                f.sources_known,
                f.has_sources,
                f.agent_listed,
                f.agent_may_use_inputs,
                f.recipients_in_audience,
                f.audience_in_readers,
            ];
            if let Some(first) = flags.iter().position(|ok| !ok) {
                assert_eq!(gate(f), ordered[first], "{f:?}");
            }
        }
    }

    #[test]
    fn denial_flag_and_codes() {
        for denial in [
            Gate::UnknownAction,
            Gate::UnknownAgent,
            Gate::UnknownSource,
            Gate::EmptyOutput,
            Gate::AgentNotListed,
            Gate::AgentCannotUseSource,
            Gate::RecipientsOutsideAudience,
            Gate::AudienceOutsideReaders,
        ] {
            assert!(denial.is_denial(), "{denial:?}");
        }
        assert!(!Gate::NeedsApproval.is_denial());
        assert!(!Gate::Allowed.is_denial());
        let codes = [
            (Gate::UnknownAction, "unknown_action"),
            (Gate::UnknownAgent, "unknown_agent"),
            (Gate::UnknownSource, "unknown_source"),
            (Gate::EmptyOutput, "empty_output"),
            (Gate::AgentNotListed, "agent_not_listed_for_action"),
            (Gate::AgentCannotUseSource, "agent_cannot_use_source"),
            (
                Gate::RecipientsOutsideAudience,
                "recipients_outside_audience",
            ),
            (Gate::AudienceOutsideReaders, "audience_outside_readers"),
            (Gate::NeedsApproval, "needs_approval"),
            (Gate::Allowed, "allowed"),
        ];
        for (gate, code) in codes {
            assert_eq!(gate.code(), code);
        }
    }

    #[test]
    fn approval_window_edges() {
        let (issued, valid) = (100u64, 50u64);
        assert!(approval_live(issued, valid, issued, false));
        assert!(approval_live(issued, valid, issued + valid - 1, false));
        assert!(!approval_live(issued, valid, issued - 1, false));
        assert!(!approval_live(issued, valid, issued + valid, false));
        assert!(!approval_live(issued, valid, issued, true));
        assert!(!approval_live(issued, 0, issued, false));
        assert!(approval_live(u64::MAX, u64::MAX, u64::MAX, false));
        assert!(approval_live(u64::MAX - 1, 5, u64::MAX, false));
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_facts() -> Facts {
        Facts {
            action_known: kani::any(),
            agent_known: kani::any(),
            sources_known: kani::any(),
            has_sources: kani::any(),
            agent_listed: kani::any(),
            agent_may_use_inputs: kani::any(),
            recipients_in_audience: kani::any(),
            audience_in_readers: kani::any(),
            approval_required: kani::any(),
            approval_live: kani::any(),
        }
    }

    #[kani::proof]
    fn gate_allows_only_when_every_fact_holds() {
        let f = any_facts();
        let permitted = f.action_known
            && f.agent_known
            && f.sources_known
            && f.has_sources
            && f.agent_listed
            && f.agent_may_use_inputs
            && f.recipients_in_audience
            && f.audience_in_readers;
        let allowed = permitted && (!f.approval_required || f.approval_live);
        assert_eq!(gate(f) == Gate::Allowed, allowed);
    }

    #[kani::proof]
    fn approval_cannot_turn_a_denial_into_allow() {
        let f = any_facts();
        let required: bool = kani::any();
        let live: bool = kani::any();
        let g = gate(f);
        if g.is_denial() {
            let varied = Facts {
                approval_required: required,
                approval_live: live,
                ..f
            };
            assert_eq!(gate(varied), g);
            assert!(gate(varied) != Gate::Allowed);
        }
    }

    #[kani::proof]
    fn needs_approval_implies_otherwise_allowed() {
        let f = any_facts();
        let permitted = f.action_known
            && f.agent_known
            && f.sources_known
            && f.has_sources
            && f.agent_listed
            && f.agent_may_use_inputs
            && f.recipients_in_audience
            && f.audience_in_readers;
        assert_eq!(
            gate(f) == Gate::NeedsApproval,
            permitted && f.approval_required && !f.approval_live
        );
        if gate(f) == Gate::NeedsApproval {
            let without_approval = Facts {
                approval_required: false,
                ..f
            };
            assert_eq!(gate(without_approval), Gate::Allowed);
        }
    }

    #[kani::proof]
    fn approval_window_is_exact_and_overflow_free() {
        let issued_at: u64 = kani::any();
        let valid_for: u64 = kani::any();
        let now: u64 = kani::any();
        let expected = (issued_at as u128) <= (now as u128)
            && (now as u128) < (issued_at as u128) + (valid_for as u128);
        assert_eq!(approval_live(issued_at, valid_for, now, false), expected);
        assert!(!approval_live(issued_at, valid_for, now, true));
    }
}
