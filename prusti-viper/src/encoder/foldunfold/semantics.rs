// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use encoder::foldunfold::perm::*;
use encoder::foldunfold::state::*;
use encoder::vir;
use std::collections::HashMap;
use std::collections::HashSet;

impl vir::Stmt {
    pub fn apply_on_state(&self, state: &mut State, predicates: &HashMap<String, vir::Predicate>, dropped: &mut HashSet<Perm>) {
        debug!("apply_on_state '{}'", self);
        trace!("State acc {{{}}}", state.display_acc());
        trace!("State pred {{{}}}", state.display_pred());
        trace!("State moved {{{}}}", state.display_moved());
        match self {
            &vir::Stmt::Comment(_) |
            &vir::Stmt::Label(_) |
            &vir::Stmt::Assert(_, _) |
            &vir::Stmt::Obtain(_) |
            &vir::Stmt::WeakObtain(_) => {}

            &vir::Stmt::Inhale(ref expr) => {
                state.insert_all_perms(
                    expr.get_permissions(predicates).into_iter()
                        .filter(|p| !(p.is_acc() && p.get_place().is_base()))
                );
            }

            &vir::Stmt::Exhale(ref expr, _) => {
                state.remove_all_perms(
                    expr.get_permissions(predicates).iter()
                        .filter(|p| !(p.is_acc() && p.get_place().is_base()))
                );
            }

            &vir::Stmt::MethodCall(_, _, ref targets) => {
                // We know that in Prusti method's preconditions and postconditions are empty
                dropped.extend(
                    state.pred().iter()
                        .filter(|(p, _)| targets.contains(p.base()))
                        .map(|(p, frac)| Perm::Pred(p.clone(), *frac))
                );
                dropped.extend(
                    state.acc().iter()
                        .filter(|(p, _)| !p.is_base() && targets.contains(p.base()))
                        .map(|(p, frac)| Perm::Acc(p.clone(), *frac))
                );
                state.remove_moved_matching(|p| targets.contains(p.base()));
                state.remove_pred_matching_place(|p| targets.contains(p.base()));
                state.remove_acc_matching_place(|p| !p.is_base() && targets.contains(p.base()));
            }

            &vir::Stmt::Assign(ref lhs_place, ref rhs, kind) => {
                let original_state = state.clone();

                // Mark the `rhs` as moved or borrowed
                match kind {
                    vir::AssignKind::Move => {
                        if let &vir::Expr::Place(ref rhs_place) = rhs {
                            assert!(rhs_place.get_type().is_ref());

                            // Check that the rhs contains no moved paths
                            assert!(!state.is_prefix_of_some_moved(&rhs_place));
                            for prefix in rhs_place.all_proper_prefixes() {
                                assert!(!state.contains_pred(prefix));
                            }

                            state.insert_moved(rhs_place.clone());
                        } else {
                            unreachable!()
                        }
                    }
                    vir::AssignKind::MutableBorrow => {
                        if let &vir::Expr::Place(ref rhs_place) = rhs {
                            assert!(rhs_place.get_type().is_ref());

                            // Check that the rhs contains no moved paths
                            assert!(!state.is_prefix_of_some_moved(&rhs_place));
                            for prefix in rhs_place.all_proper_prefixes() {
                                assert!(!state.contains_pred(prefix));
                            }
                        } else {
                            unreachable!()
                        }
                    }
                    _ => {}
                }

                // Remove places that will not have a name
                dropped.extend(
                    state.pred().iter()
                        .filter(|(p, _)| p.has_prefix(&lhs_place))
                        .map(|(p, frac)| Perm::Pred(p.clone(), *frac))
                );
                dropped.extend(
                    state.acc().iter()
                        .filter(|(p, _)| p.has_proper_prefix(&lhs_place))
                        .map(|(p, frac)| Perm::Acc(p.clone(), *frac))
                );
                state.remove_moved_matching( |p| p.has_prefix(&lhs_place));
                state.remove_pred_matching_place( |p| p.has_prefix(&lhs_place));
                state.remove_acc_matching_place( |p| p.has_proper_prefix(&lhs_place));

                // In case of move or borrowing, move permissions from the `rhs` to the `lhs`
                match rhs {
                    &vir::Expr::Place(ref rhs_place) if rhs_place.get_type().is_ref() => {
                        // This is a move assignemnt or the creation of a mutable borrow
                        assert!(match kind { vir::AssignKind::Copy => false, _ => true }, "Unexpected assignment kind: {:?}", kind);

                        // In Prusti, we lose permission on the rhs
                        state.remove_pred_matching_place( |p| p.has_prefix(&rhs_place));
                        state.remove_acc_matching_place( |p| p.has_proper_prefix(&rhs_place));

                        // We also lose permission on the lhs
                        state.remove_pred_matching_place( |p| p.has_prefix(&lhs_place));
                        state.remove_acc_matching_place( |p| p.has_prefix(&lhs_place));

                        // And we create permissions for the lhs
                        let new_acc_places = original_state.acc().iter()
                            .filter(|(p, _)| p.has_prefix(&rhs_place))
                            .map(|(p, frac)| (p.clone().replace_prefix(&rhs_place, lhs_place.clone()), *frac));
                        state.insert_all_acc(new_acc_places);

                        let new_pred_places = original_state.pred().iter()
                            .filter(|(p, _)| p.has_prefix(&rhs_place))
                            .map(|(p, frac)| (p.clone().replace_prefix(&rhs_place, lhs_place.clone()), *frac));
                        state.insert_all_pred(new_pred_places);
                    }
                    _ => {
                        // This is not move assignemnt or the creation of a mutable borrow
                        assert!(match kind { vir::AssignKind::Copy => true, _ => false }, "Unexpected assignment kind: {:?}", kind);
                    }
                }
            }

            &vir::Stmt::Fold(ref pred_name, ref args, frac) => {
                assert_eq!(args.len(), 1);
                let place = &args[0].clone().as_place().unwrap();
                assert!(!state.contains_pred(&place));
                assert!(!state.is_prefix_of_some_moved(&place));

                // We want to fold place
                let predicate_name = place.typed_ref_name().unwrap();
                let predicate = predicates.get(&predicate_name).unwrap();

                let pred_self_place: vir::Place = predicate.args[0].clone().into();
                let places_in_pred: Vec<Perm> = predicate.get_permissions().into_iter()
                    .map(
                        |perm| {
                            perm.map_place( |p|
                                p.replace_prefix(&pred_self_place, place.clone())
                            ) * frac
                        }
                    ).collect();

                // Commented due to the presence of implications in the body of predicates
                //for contained_place in &places_in_pred {
                //    assert!(state.contains(contained_place));
                //}

                // Simulate folding of `place`
                state.remove_all_perms(places_in_pred.iter());
                state.insert_pred(place.clone(), frac);
            }

            &vir::Stmt::Unfold(ref pred_name, ref args, frac) => {
                assert_eq!(args.len(), 1);
                let place = &args[0].clone().as_place().unwrap();
                assert!(state.contains_pred(&place));
                assert!(!state.is_prefix_of_some_moved(&place));

                // We want to unfold place
                let predicate_name = place.typed_ref_name().unwrap();
                let predicate = predicates.get(&predicate_name).unwrap();

                let pred_self_place: vir::Place = predicate.args[0].clone().into();
                let places_in_pred: Vec<Perm> = predicate.get_permissions().into_iter()
                    .map( |aop| aop.map_place( |p|
                        p.replace_prefix(&pred_self_place, place.clone())
                    )).collect();

                for contained_place in &places_in_pred {
                    assert!(!state.contains_perm(contained_place));
                }

                // Simulate unfolding of `place`
                state.remove_pred(&place, frac);
                state.insert_all_perms(places_in_pred.into_iter());
            }


            &vir::Stmt::Havoc => {
                state.remove_matching(|p| !p.is_base());
            }

            &vir::Stmt::BeginFrame => {
                state.begin_frame()
            }

            &vir::Stmt::EndFrame => {
                state.end_frame()
            }

            &vir::Stmt::ExpireBorrow(ref lhs_place, ref rhs_place) => {
                let original_state = state.clone();

                assert!(lhs_place.get_type().is_ref());
                assert!(rhs_place.get_type().is_ref());
                assert_eq!(lhs_place.get_type(), rhs_place.get_type());
                assert!(!state.is_proper_prefix_of_some_acc(&rhs_place));
                assert!(!state.is_prefix_of_some_pred(&rhs_place));
                assert!(!state.is_prefix_of_some_moved(&rhs_place));
                assert!(!state.is_prefix_of_some_moved(&lhs_place));

                // Restore permissions from the `lhs` to the `rhs`

                // This is the creation of a mutable borrow

                // In Prusti, lose permission from the lhs and rhs
                state.remove_pred_matching_place(|p| p.has_prefix(&lhs_place));
                state.remove_acc_matching_place(|p| p.has_proper_prefix(&lhs_place));
                state.remove_pred_matching_place(|p| p.has_prefix(&rhs_place));
                state.remove_acc_matching_place(|p| p.has_proper_prefix(&rhs_place));

                // The rhs is no longer moved
                state.remove_moved_matching(|p| p.has_prefix(&rhs_place));

                // And we create permissions for the rhs
                let new_acc_places = original_state.acc().iter()
                    .filter(|(p, _)| p.has_prefix(&lhs_place))
                    .map(|(p, frac)| (p.clone().replace_prefix(&lhs_place, rhs_place.clone()), *frac));
                state.insert_all_acc(new_acc_places);

                let new_pred_places = original_state.pred().iter()
                    .filter(|(p, _)| p.has_prefix(&lhs_place))
                    .map(|(p, frac)| (p.clone().replace_prefix(&lhs_place, rhs_place.clone()), *frac));
                state.insert_all_pred(new_pred_places);

            }
        }
    }
}
