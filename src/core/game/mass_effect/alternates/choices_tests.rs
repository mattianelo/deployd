use super::*;

fn options(format: &str, rules: &str) -> Result<choices::Model> {
    let text = format!(
        "((FriendlyName=A,OptionKey=A,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=B,OptionKey=B,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=C,OptionKey=C,Condition=COND_MANUAL,ModOperation=OP_NOTHING,{rules}))"
    );
    choices::Model::new(&Alternate::parse(Some(&text), true, Target::Le1)?, format)
}

// @variants: both
#[test]
fn dependencies_follow_signed_keys_and_versioned_selection_transitions() -> Result<()> {
    let rules = "DependsOnKeys=+A;-B,DependsOnMetAction=ACTION_ALLOW_SELECT_CHECKED,DependsOnNotMetAction=ACTION_DISALLOW_SELECT";
    let model = options("9.2", rules)?;
    let definitions = Alternate::parse(
        Some(
            "((FriendlyName=A,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=B,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=C,Condition=COND_MANUAL,ModOperation=OP_NOTHING))",
        ),
        true,
        Target::Le1,
    )?;
    let (a, b, c) = (
        &definitions[0].key,
        &definitions[1].key,
        &definitions[2].key,
    );
    let blocked = model.evaluate(&BTreeSet::from([c.clone()]), None)?;
    assert!(!blocked.selected.contains(c));
    assert!(!blocked.selectable.contains(c));
    let unlocked = model.evaluate(&BTreeSet::from([a.clone()]), Some(&blocked))?;
    assert!(unlocked.selected.contains(c));
    assert!(unlocked.selectable.contains(c));
    let manual = model.evaluate(&BTreeSet::from([a.clone()]), Some(&unlocked))?;
    assert!(!manual.selected.contains(c));
    let denied = model.evaluate(
        &BTreeSet::from([a.clone(), b.clone(), c.clone()]),
        Some(&unlocked),
    )?;
    assert!(!denied.selected.contains(c));
    let legacy = options("9.1", rules)?.evaluate(&BTreeSet::from([a.clone()]), Some(&blocked))?;
    assert!(!legacy.selected.contains(c));
    let force = options(
        "9.2",
        "DependsOnKeys=+A,DependsOnMetAction=ACTION_DISALLOW_SELECT_CHECKED,DependsOnNotMetAction=ACTION_ALLOW_SELECT",
    )?;
    let state = force.evaluate(&BTreeSet::from([a.clone()]), None)?;
    assert!(state.selected.contains(c));
    assert!(!state.selectable.contains(c));
    let released = force.evaluate(&BTreeSet::new(), Some(&state))?;
    assert!(!released.selected.contains(c));
    assert!(released.selectable.contains(c));
    Ok(())
}

// @variants: both
#[test]
fn dependency_graph_rejects_unknown_keys_cycles_and_unsupported_versions() -> Result<()> {
    let rules = "DependsOnKeys=+A,DependsOnMetAction=ACTION_ALLOW_SELECT,DependsOnNotMetAction=ACTION_DISALLOW_SELECT";
    assert!(options("7.0", rules).is_err());
    for invalid in [
        rules.replace("+A", "+Missing"),
        rules.replace("+A", "+C"),
        rules.replace("+A", "A"),
        rules.replace("+A", "+A;-A"),
        rules.replace("ACTION_ALLOW_SELECT", "ACTION_FUTURE"),
    ] {
        assert!(options("9.2", &invalid).is_err());
    }
    let dashed = format!(
        "((FriendlyName=A,OptionKey=A-choice,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=C,OptionKey=C,Condition=COND_MANUAL,ModOperation=OP_NOTHING,{}))",
        rules.replace("+A", "+A-choice")
    );
    choices::Model::new(&Alternate::parse(Some(&dashed), true, Target::Le1)?, "9.2")?;
    let cyclic = format!(
        "((FriendlyName=A,OptionKey=A,Condition=COND_MANUAL,ModOperation=OP_NOTHING,{}),(FriendlyName=C,OptionKey=C,Condition=COND_MANUAL,ModOperation=OP_NOTHING,{rules}))",
        rules.replace("+A", "+C")
    );
    let parsed = Alternate::parse(Some(&cyclic), true, Target::Le1)?;
    assert!(choices::Model::new(&parsed, "9.2").is_err());
    let grouped = format!(
        "((FriendlyName=A,OptionKey=A,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=C,OptionKey=C,OptionGroup=G,Condition=COND_MANUAL,ModOperation=OP_NOTHING,{rules}))"
    );
    choices::Model::new(&Alternate::parse(Some(&grouped), true, Target::Le1)?, "9.2")?;
    Ok(())
}

// @variants: both
#[test]
fn generated_dependency_keys_follow_unicode_names_and_groups() -> Result<()> {
    for (name, group, expected) in [
        ("A", "", "BB6CBBA8"),
        ("Face", "Appearance", "4685EFDD"),
        ("Jóan👩", "", "8E6AC4CE"),
    ] {
        let group = if group.is_empty() {
            String::new()
        } else {
            format!(",OptionGroup={group}")
        };
        let text = format!(
            "((FriendlyName={name}{group},Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=Dependent,Condition=COND_MANUAL,ModOperation=OP_NOTHING,DependsOnKeys=+{expected},DependsOnMetAction=ACTION_DISALLOW_SELECT_CHECKED,DependsOnNotMetAction=ACTION_DISALLOW_SELECT))"
        );
        let mut definitions = Alternate::parse(Some(&text), true, Target::Le1)?;
        let mut initially_selected = BTreeSet::new();
        if !group.is_empty() {
            let other = Alternate::parse(Some(&format!("((FriendlyName=Other{group},CheckedByDefault=true,Condition=COND_MANUAL,ModOperation=OP_NOTHING))")), true, Target::Le1)?.remove(0);
            initially_selected.insert(other.key.clone());
            definitions.push(other);
        }
        assert_eq!(definitions[0].reference_key(), expected);
        let model = choices::Model::new(&definitions, "9.2")?;
        assert!(
            !model
                .evaluate(&initially_selected, None)?
                .selected
                .contains(&definitions[1].key)
        );
        assert!(
            model
                .evaluate(&BTreeSet::from([definitions[0].key.clone()]), None)?
                .selected
                .contains(&definitions[1].key)
        );
    }
    Ok(())
}

// @variants: both
#[test]
fn generated_key_collisions_preserve_legacy_manifest_acceptance() -> Result<()> {
    let text = "((FriendlyName=Same,Condition=COND_MANUAL,ModOperation=OP_NOTHING))";
    let mut definitions = Alternate::parse(Some(text), true, Target::Le1)?;
    definitions.extend(Alternate::parse(Some(text), false, Target::Le1)?);
    choices::Model::new(&definitions, "7.0")?;
    assert!(choices::Model::new(&definitions, "8.0").is_err());
    assert!(choices::Model::new(&definitions, "9.2").is_err());
    Ok(())
}

// @variants: both
#[test]
fn contextual_dependencies_resolve_dlc_versions_options_and_groups() -> Result<()> {
    let text = "((FriendlyName=Automatic,OptionKey=Auto,Condition=COND_SPECIFIC_DLC_SETUP,ConditionalDLC=+DLC_MOD_A[minversion=2.0,maxversion=3.0,optionkey=[option=+Chosen,uistring=Chosen variant]];-DLC_MOD_B,ModOperation=OP_NOTHING),(FriendlyName=Default,OptionGroup=Variant,CheckedByDefault=true,Condition=COND_MANUAL,ModOperation=OP_NOTHING),(FriendlyName=Compatible,OptionGroup=Variant,Condition=COND_MANUAL,DependsOnKeys=+Auto,DependsOnMetAction=ACTION_DISALLOW_SELECT_CHECKED,DependsOnNotMetAction=ACTION_DISALLOW_SELECT,ModOperation=OP_NOTHING))";
    let definitions = Alternate::parse(Some(text), true, Target::Le1)?;
    let model = choices::Model::new(&definitions, "9.2")?;
    let selected = BTreeSet::from([definitions[1].key.clone()]);
    let available = BTreeSet::from(["dlc_mod_a".into()]);
    let versions = BTreeMap::from([("dlc_mod_a".into(), "2.5".into())]);
    let mut options = BTreeMap::from([("dlc_mod_a".into(), BTreeSet::from(["chosen".into()]))]);
    let sizes = BTreeMap::new();
    let state = model.evaluate_context(
        &selected,
        None,
        Some(&Context {
            available: &available,
            sizes: &sizes,
            versions: Some(&versions),
            options: Some(&options),
        }),
    )?;
    assert!(state.selected.contains(&definitions[0].key));
    assert!(state.selected.contains(&definitions[2].key));
    assert!(!state.selected.contains(&definitions[1].key));
    options.clear();
    options.insert("dlc_mod_a".into(), BTreeSet::new());
    let state = model.evaluate_context(
        &selected,
        None,
        Some(&Context {
            available: &available,
            sizes: &sizes,
            versions: Some(&versions),
            options: Some(&options),
        }),
    )?;
    assert!(!state.selected.contains(&definitions[0].key));
    assert!(!state.selected.contains(&definitions[2].key));
    assert!(state.selected.contains(&definitions[1].key));
    Ok(())
}
