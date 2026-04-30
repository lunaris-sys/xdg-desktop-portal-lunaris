/// Picker state: the currently-active request and its UI state.
///
/// The store lives at app-state level (not per-component) because the
/// daemon can deliver a new request at any time and the UI needs to
/// react globally. Multiple in-flight requests are not supported by
/// the picker UI itself — the daemon serializes them per FA1/E1.

import type { PickerRequest } from "$lib/types/protocol";

interface PickState {
  request: PickerRequest | null;
  busy: boolean;
}

const state = $state<PickState>({
  request: null,
  busy: false,
});

export function getPickState(): PickState {
  return state;
}

export function setRequest(req: PickerRequest | null) {
  state.request = req;
  state.busy = false;
}

export function setBusy(b: boolean) {
  state.busy = b;
}
