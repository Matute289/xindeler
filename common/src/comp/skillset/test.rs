use super::*;

// Unneeded cfg(test) here keeps rust-analyzer happy
#[cfg(test)]
use petgraph::{algo::is_cyclic_directed, graph::DiGraph};

#[test]
fn check_cyclic_skill_deps() {
    let skill_prereqs: HashMap<Skill, SkillPrerequisite> =
        Ron::load_expect_cloned("common.skill_trees.skill_prerequisites").0;
    let mut graph = DiGraph::new();
    let mut nodes = HashMap::<Skill, _>::new();
    let mut add_node = |graph: &mut DiGraph<Skill, _>, node: Skill| {
        *nodes.entry(node).or_insert_with(|| graph.add_node(node))
    };

    for (skill, prereqs) in skill_prereqs.iter() {
        let skill_node = add_node(&mut graph, *skill);
        let prereqs = match prereqs {
            SkillPrerequisite::Any(skills) => skills,
            SkillPrerequisite::All(skills) => skills,
        };
        for (prereq, _) in prereqs.iter() {
            let prereq_node = add_node(&mut graph, *prereq);
            graph.add_edge(prereq_node, skill_node, ());
        }
    }

    assert!(!is_cyclic_directed(&graph));
}

// ---- BL-06 class skill trees ----

#[test]
fn class_skill_persistence_round_trip() {
    use crate::comp::skills::{MageSkill, WarriorSkill};
    // Class skills persist via serde (json) like weapon skills; a new variant
    // must round-trip without a manual conversion arm.
    let skills = vec![
        Skill::Warrior(WarriorSkill::Onslaught),
        Skill::Mage(MageSkill::FocusedMind),
        Skill::Warrior(WarriorSkill::HardenedBody),
    ];
    let json = serde_json::to_string(&skills).expect("serialize class skills");
    let back: Vec<Skill> = serde_json::from_str(&json).expect("deserialize class skills");
    assert_eq!(skills, back);
}

#[test]
fn class_passive_raises_stats_field() {
    use crate::comp::{Body, Stats, body::humanoid, skills::WarriorSkill};

    let body = Body::Humanoid(humanoid::Body::iter().next().expect("a humanoid body"));
    let mut skillset = SkillSet::default();
    // Seed a leveled passive directly (the unlock flow is covered elsewhere).
    skillset
        .skills
        .insert(Skill::Warrior(WarriorSkill::HardenedBody), 2);

    let mut stats = Stats::empty(body);
    let before = stats.max_health_modifiers.mult_mod;
    skillset.apply_class_passives(&mut stats);
    // HardenedBody = +0.04 max-health per level; level 2 → *= 1.08.
    assert!((stats.max_health_modifiers.mult_mod - before * 1.08).abs() < 1e-5);
}

#[test]
fn caster_passives_use_spell_power_channel() {
    use crate::comp::{Body, Stats, body::humanoid, skills::MageSkill};

    let body = Body::Humanoid(humanoid::Body::iter().next().expect("a humanoid body"));
    let mut skillset = SkillSet::default();
    // SpellPotency is re-pointed to the magic-only `spell_power` channel (Q2/Q3)
    // so it must NOT touch the global physical `attack_damage_modifier`.
    skillset
        .skills
        .insert(Skill::Mage(MageSkill::SpellPotency), 3);

    let mut stats = Stats::empty(body);
    let attack_before = stats.attack_damage_modifier;
    skillset.apply_class_passives(&mut stats);
    // SpellPotency = +0.04 spell_power per level; level 3 → *= 1.12.
    assert!((stats.spell_power - 1.12).abs() < 1e-5);
    assert_eq!(
        stats.attack_damage_modifier, attack_before,
        "caster damage passive must not leak onto physical attack_damage_modifier",
    );
}

#[test]
fn heal_power_passive_applies() {
    use crate::comp::{Stats, body::humanoid, skills::ClassPassiveStat};

    let body = crate::comp::Body::Humanoid(humanoid::Body::iter().next().unwrap());
    let mut stats = Stats::empty(body);
    ClassPassiveStat::HealPower.apply(&mut stats, 0.2);
    assert!((stats.heal_power - 1.2).abs() < 1e-5);
}

#[test]
fn active_skills_have_no_passive_modifier() {
    use crate::comp::skills::{ClericSkill, MageSkill, RogueSkill, WarriorSkill};
    // The 8 signature/capstone actives unlock abilities, not passive stats —
    // they must be absent from the modifier manifest.
    for active in [
        Skill::Warrior(WarriorSkill::Rally),
        Skill::Warrior(WarriorSkill::Onslaught),
        Skill::Mage(MageSkill::ArcaneSurge),
        Skill::Mage(MageSkill::ArcaneMastery),
        Skill::Cleric(ClericSkill::MendingLight),
        Skill::Cleric(ClericSkill::RadiantChannel),
        Skill::Rogue(RogueSkill::Ambush),
        Skill::Rogue(RogueSkill::Vanish),
    ] {
        assert!(
            CLASS_SKILL_MODIFIERS.get(&active).is_none(),
            "{active:?} is an active ability and must not have a passive modifier",
        );
    }
}

#[test]
fn class_skill_modifiers_manifest_integrity() {
    // Every modifier entry must be a real skill living in a Class skill group.
    for skill in CLASS_SKILL_MODIFIERS.keys() {
        let group = SKILL_GROUP_LOOKUP
            .get(skill)
            .unwrap_or_else(|| panic!("{skill:?} has a modifier but is in no skill group"));
        assert!(
            matches!(group, SkillGroupKind::Class(_)),
            "{skill:?} modifier must belong to a Class group, got {group:?}",
        );
    }
}
