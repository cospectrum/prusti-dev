// © 2021, ETH Zurich
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use crate::{
    abstract_interpretation::{AnalysisResult, FixpointEngine},
    domains::{
        DefinitelyAccessibleState, DefinitelyInitializedAnalysis, DefinitelyInitializedState,
        MaybeBorrowedAnalysis, MaybeBorrowedState,
    },
    PointwiseState,
};
use rustc_borrowck::BodyWithBorrowckFacts;
use rustc_middle::{mir, ty::TyCtxt};
use rustc_span::def_id::DefId;
use rustc_data_structures::stable_set::FxHashSet;
use std::mem;
use crate::mir_utils::{is_prefix, expand};

pub struct DefinitelyAccessibleAnalysis<'mir, 'tcx: 'mir> {
    tcx: TyCtxt<'tcx>,
    def_id: DefId,
    body_with_facts: &'mir BodyWithBorrowckFacts<'tcx>,
}

impl<'mir, 'tcx: 'mir> DefinitelyAccessibleAnalysis<'mir, 'tcx> {
    pub fn new(
        tcx: TyCtxt<'tcx>,
        def_id: DefId,
        body_with_facts: &'mir BodyWithBorrowckFacts<'tcx>,
    ) -> Self {
        DefinitelyAccessibleAnalysis {
            tcx,
            def_id,
            body_with_facts,
        }
    }

    pub fn run_analysis<'body>(
        &'body self,
    ) -> AnalysisResult<PointwiseState<'body, 'tcx, DefinitelyAccessibleState<'tcx>>> {
        let body = &self.body_with_facts.body;
        let def_init_analysis =
            DefinitelyInitializedAnalysis::new(self.tcx, self.def_id, body);
        let borrowed_analysis = MaybeBorrowedAnalysis::new(self.tcx, &self.body_with_facts);
        let def_init = def_init_analysis.run_fwd_analysis()?;
        let borrowed = borrowed_analysis.run_analysis()?;
        let mut analysis_state = PointwiseState::default(body);

        // Set state_after_block
        for (block, block_data) in body.basic_blocks().iter_enumerated() {
            // Initialize the state before each statement
            for statement_index in 0..=block_data.statements.len() {
                let location = mir::Location {
                    block,
                    statement_index,
                };
                let def_init_before = def_init
                    .lookup_before(location)
                    .unwrap_or_else(|| panic!("No 'def_init' state after location {:?}", location));
                let borrowed_before = borrowed
                    .lookup_before(location)
                    .unwrap_or_else(|| panic!("No 'borrowed' state after location {:?}", location));
                let state = self.compute_accessible_state(def_init_before, borrowed_before);
                state.check_invariant();
                analysis_state.set_before(location, state);
            }

            // Initialize the state of successors of terminators
            let def_init_after_block = def_init
                .lookup_after_block(block)
                .unwrap_or_else(|| panic!("No 'def_init' state after block {:?}", block));
            let borrowed_after_block = borrowed
                .lookup_after_block(block)
                .unwrap_or_else(|| panic!("No 'borrowed' state after block {:?}", block));
            let available_after_block = analysis_state.lookup_mut_after_block(block);
            for &successor in block_data.terminator().successors() {
                let def_init_after = def_init_after_block.get(&successor).unwrap_or_else(|| {
                    panic!("No 'def_init' state from {:?} to {:?}", block, successor)
                });
                let borrowed_after = borrowed_after_block.get(&successor).unwrap_or_else(|| {
                    panic!("No 'borrowed' state from {:?} to {:?}", block, successor)
                });
                let state = self.compute_accessible_state(def_init_after, borrowed_after);
                state.check_invariant();
                available_after_block.insert(successor, state);
            }
        }

        Ok(analysis_state)
    }

    /// Compute the `definitely_available` state by subtracting the `maybe_borrowed` state from
    /// the `definitely_initialized` one.
    fn compute_accessible_state(
        &self,
        def_init: &DefinitelyInitializedState<'mir, 'tcx>,
        borrowed: &MaybeBorrowedState<'tcx>,
    ) -> DefinitelyAccessibleState<'tcx> {
        let mut definitely_accessible: FxHashSet<_> = def_init.get_def_init_places().clone();
        for &place in borrowed.get_maybe_mut_borrowed().iter() {
            self.remove_place_from_set(place, &mut definitely_accessible);
        }
        let mut definitely_owned = definitely_accessible.clone();
        for &place in borrowed.get_maybe_shared_borrowed().iter() {
            self.remove_place_from_set(place, &mut definitely_owned);
        }
        DefinitelyAccessibleState {
            definitely_accessible,
            definitely_owned,
        }
    }

    /// Remove all extensions of a place from a set, unpacking prefixes as much as needed.
    fn remove_place_from_set(&self, to_remove: mir::Place<'tcx>, set: &mut FxHashSet<mir::Place<'tcx>>) {
        let old_set = mem::take(set);
        for old_place in old_set {
            if is_prefix(to_remove, old_place) {
                // Unpack `old_place` and remove `to_remove`.
                set.extend(expand(&self.body_with_facts.body, self.tcx, old_place, to_remove));
            } else if is_prefix(old_place, to_remove) {
                // `to_remove` is a prefix of `old_place`. So, it should *not* be added to `set`.
            } else {
                // `old_place` and `to_remove` are unrelated.
                set.insert(old_place);
            }
        }
    }
}
