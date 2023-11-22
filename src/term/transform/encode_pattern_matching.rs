use crate::term::{
  check::type_check::{DefinitionTypes, Type},
  Book, DefId, Name, Rule, RulePat, Term,
};

impl Book {
  pub fn encode_pattern_matching_functions(&mut self, def_types: &DefinitionTypes) {
    for def_id in self.defs.keys().copied().collect::<Vec<_>>() {
      let def_name = self.def_names.name(&def_id).unwrap();
      let def = self.defs.get_mut(&def_id).unwrap();

      // TODO: For functions with only one rule that doesnt pattern match, we can skip this for better readability of compiled result
      // First create a definition for each rule body
      for (rule_idx, rule) in def.rules.iter_mut().enumerate() {
        let rule_name = make_rule_name(def_name, rule_idx);
        let body = std::mem::replace(&mut rule.body, Term::Era);
        let body = make_rule_body(body, rule_idx, def_name, &rule.pats);
        self.insert_def(rule_name, vec![Rule { pats: vec![], body }]);
      }

      // Generate scott-encoded pattern matching
      let def_type = &def_types[&def_id];
      let crnt_rules = (0 .. def.rules.len()).collect();
      make_pattern_matching_case(self, def_type, def_id, def_name, crnt_rules, vec![]);
    }
  }
}

fn make_rule_name(def_name: &Name, rule_idx: usize) -> Name {
  Name(format!("{def_name}${rule_idx}"))
}

/// Given
fn make_rule_body(mut body: Term, rule_idx: usize, def_name: &Name, pats: &[RulePat]) -> Term {
  // Add the lambdas for the pattern variables
  for pat in pats.iter().rev() {
    match pat {
      RulePat::Var(nam) => body = Term::Lam { nam: Some(nam.clone()), bod: Box::new(body) },
      RulePat::Ctr(nam, vars) => {
        for var in vars.iter().rev() {
          let RulePat::Var(nam) = var else { unreachable!() };
          body = Term::Lam { nam: Some(nam.clone()), bod: Box::new(body) }
        }
      }
    }
  }
  body
}

fn make_pattern_matching_case(
  book: &mut Book,
  def_type: &[Type],
  def_id: DefId,
  crnt_name: &Name,
  crnt_rules: Vec<usize>,
  mut match_path: Vec<RulePat>,
) {
  let def = &book.defs[&def_id];
  let def_name = book.def_names.name(&def_id).unwrap();
  // This is safe since we check exhaustiveness earlier.
  let fst_rule_idx = crnt_rules[0];
  let fst_rule = &def.rules[fst_rule_idx];
  let crnt_arg_idx = match_path.len();

  // Check if we've reached the end for this subfunction.
  // We did if all the (possibly zero) remaining patterns are variables and not matches.
  let all_args_done = crnt_arg_idx >= fst_rule.arity();
  let is_fst_rule_irrefutable =
    all_args_done || fst_rule.pats[crnt_arg_idx ..].iter().all(|p| matches!(p, RulePat::Var(_)));
  if is_fst_rule_irrefutable {
    let rule_def_name = make_rule_name(def_name, fst_rule_idx);
    let rule_def_id = book.def_names.def_id(&rule_def_name).unwrap();
    make_leaf_pattern_matching_case(
      book,
      Name::new(crnt_name),
      rule_def_id,
      &fst_rule.pats.clone(),
      match_path,
    );
  } else {
    let is_matching_case =
      crnt_rules.iter().any(|rule_idx| matches!(def.rules[*rule_idx].pats[crnt_arg_idx], RulePat::Ctr(..)));
    if is_matching_case {
      make_branch_pattern_matching_case(book, def_type, def_id, crnt_name, crnt_rules, match_path);
    } else {
      // In non matching cases, we just add this argument to the variables and go to the next pattern.
      match_path.push(RulePat::Var(Name::new("")));
      make_pattern_matching_case(book, def_type, def_id, crnt_name, crnt_rules, match_path);
    }
  }
}

/// Builds the function calling one of the original rule bodies.
fn make_leaf_pattern_matching_case(
  book: &mut Book,
  new_def_name: Name,
  rule_def_id: DefId,
  rule_pats: &[RulePat],
  match_path: Vec<RulePat>,
) {
  // The term we're building
  let mut term = Term::Ref { def_id: rule_def_id };
  // Counts how many variables are used and then counts down to declare them.
  let mut matched_var_counter = 0;

  let use_var = |counter: &mut usize| {
    let nam = Name(format!("x{counter}"));
    *counter += 1;
    nam
  };
  let make_var = |counter: &mut usize| {
    let nam = Name(format!("x{counter}"));
    *counter -= 1;
    nam
  };
  let make_app = |term: Term, nam: Name| Term::App { fun: Box::new(term), arg: Box::new(Term::Var { nam }) };
  let make_lam = |nam: Name, term: Term| Term::Lam { nam: Some(nam), bod: Box::new(term) };

  // Add the applications to call the rule body
  term = match_path.iter().zip(rule_pats).fold(term, |term, (matched, pat)| {
    match (matched, pat) {
      (RulePat::Var(_), RulePat::Var(_)) => make_app(term, use_var(&mut matched_var_counter)),
      (RulePat::Ctr(_, vars), RulePat::Ctr(_, _)) => {
        vars.iter().fold(term, |term, _| make_app(term, use_var(&mut matched_var_counter)))
      }
      // This particular rule was not matching on this arg but due to the other rules we had to match on a constructor.
      // So, to call the rule body we have to recreate the constructor.
      // (On scott encoding, if one of the cases is matched we must also match on all the other constructors for this arg)
      (RulePat::Ctr(ctr_nam, vars), RulePat::Var(_)) => {
        let ctr_ref_id = book.def_names.def_id(&ctr_nam).unwrap();
        let ctr_args = vars.iter().map(|_| use_var(&mut matched_var_counter));
        let ctr_term =
          ctr_args.fold(Term::Ref { def_id: ctr_ref_id }, |ctr_term, ctr_var| make_app(ctr_term, ctr_var));
        Term::App { fun: Box::new(term), arg: Box::new(ctr_term) }
      }
      (RulePat::Var(_), RulePat::Ctr(_, _)) => unreachable!(),
    }
  });

  // Add the lambdas to get the matched variables
  term = match_path.iter().rev().fold(term, |term, matched| match matched {
    RulePat::Var(_) => make_lam(make_var(&mut matched_var_counter), term),
    RulePat::Ctr(_, vars) => {
      vars.iter().fold(term, |term, _| make_lam(make_var(&mut matched_var_counter), term))
    }
  });

  book.insert_def(new_def_name, vec![Rule { pats: vec![], body: term }]);
}

fn make_branch_pattern_matching_case(
  book: &mut Book,
  def_type: &[Type],
  def_id: DefId,
  crnt_name: &Name,
  crnt_rules: Vec<usize>,
  match_path: Vec<RulePat>,
) {
  fn filter_rules(def_rules: &[Rule], crnt_rules: &[usize], arg_idx: usize, ctr: &Name) -> Vec<usize> {
    crnt_rules
      .iter()
      .copied()
      .filter(|&rule_idx| match &def_rules[rule_idx].pats[arg_idx] {
        RulePat::Var(_) => true,
        RulePat::Ctr(nam, _) => nam == ctr,
      })
      .collect()
  }
  let make_next_fn_name = |crnt_name, ctr_name| Name(format!("{crnt_name}$P{ctr_name}"));
  let make_app = |term, arg| Term::App { fun: Box::new(term), arg: Box::new(arg) };
  let make_lam = |nam, term| Term::Lam { nam: Some(nam), bod: Box::new(term) };
  let use_var = |counter: &mut usize| {
    let nam = Name(format!("x{counter}"));
    *counter += 1;
    nam
  };
  let make_var = |counter: &mut usize| {
    let nam = Name(format!("x{counter}"));
    *counter -= 1;
    nam
  };

  let crnt_arg_idx = match_path.len();
  let Type::Adt(next_type) = &def_type[crnt_arg_idx] else { unreachable!() };
  let next_ctrs = book.adts[next_type].ctrs.clone();

  // First we create the subfunctions
  // TODO: We could group together functions with same arity that map to the same (default) case.
  for (next_ctr, &next_ctr_ari) in next_ctrs.iter() {
    let def = &book.defs[&def_id];
    let crnt_name = make_next_fn_name(crnt_name, next_ctr);
    let crnt_rules = filter_rules(&def.rules, &crnt_rules, match_path.len(), next_ctr);
    let new_vars = RulePat::Ctr(next_ctr.clone(), vec![RulePat::Var(Name::new("")); next_ctr_ari]);
    let mut match_path = match_path.clone();
    match_path.push(new_vars);
    make_pattern_matching_case(book, def_type, def_id, &crnt_name, crnt_rules, match_path);
  }

  // Pattern matched value
  let term = Term::Var { nam: Name::new("x") };
  let term = next_ctrs.keys().fold(term, |term, ctr| {
    let name = make_next_fn_name(crnt_name, ctr);
    let def_id = book.def_names.def_id(&name).unwrap();
    make_app(term, Term::Ref { def_id })
  });
  let mut var_count = 0;
  // Applied arguments
  let term = match_path.iter().fold(term, |term, pat| match pat {
    RulePat::Var(_) => make_app(term, Term::Var { nam: use_var(&mut var_count) }),
    RulePat::Ctr(_, vars) => {
      vars.iter().fold(term, |term, _| make_app(term, Term::Var { nam: use_var(&mut var_count) }))
    }
  });
  // Lambdas for arguments
  let term = match_path.iter().rev().fold(term, |term, pat| match pat {
    RulePat::Var(_) => make_lam(make_var(&mut var_count), term),
    RulePat::Ctr(_, vars) => vars.iter().fold(term, |term, _| make_lam(make_var(&mut var_count), term)),
  });
  // Lambda for the matched variable
  let term = Term::Lam { nam: Some(Name::new("x")), bod: Box::new(term) };
  book.insert_def(Name::new(crnt_name), vec![Rule{ pats: vec![], body: term }]);
}

fn get_next_fn_names<'a>(crnt_name: &str, ctrs: impl Iterator<Item = &'a Name>) -> Vec<Name> {
  ctrs.map(|ctr| Name(format!("{}${}", crnt_name, ctr))).collect()
}
