/// TypeScript shapes for the daemon ↔ picker-ui IPC, mirroring
/// `protocol/src/lib.rs` in the workspace. Field names use camelCase
/// because the Rust side has `serde(rename_all = "camelCase")` and
/// `rename_all_fields = "camelCase"`.
///
/// Keeping these types in lockstep with the Rust definitions is a
/// manual discipline; F2.5 includes a contract test that submits a
/// real frame both ways to catch drift.

export type FilterPattern =
  | { kind: "glob"; pattern: string }
  | { kind: "mime"; mimeType: string };

export interface FileFilter {
  name: string;
  patterns: FilterPattern[];
}

export type PickerRequest =
  | {
      type: "openFile";
      handle: string;
      appId: string;
      title: string;
      filters: FileFilter[];
      currentFilter: FileFilter | null;
      multiple: boolean;
      modal: boolean;
      currentFolder: string | null;
      parentWindow: string | null;
    }
  | {
      type: "saveFile";
      handle: string;
      appId: string;
      title: string;
      filters: FileFilter[];
      currentFilter: FileFilter | null;
      currentName: string | null;
      currentFolder: string | null;
      currentFile: string | null;
      parentWindow: string | null;
    }
  | {
      type: "saveFiles";
      handle: string;
      appId: string;
      title: string;
      files: string[];
      currentFolder: string | null;
      parentWindow: string | null;
    }
  | { type: "cancel"; handle: string };

export type PickerResponse =
  | {
      type: "picked";
      handle: string;
      paths: string[];
      currentFilter: FileFilter | null;
    }
  | { type: "cancelled"; handle: string }
  | { type: "error"; handle: string; message: string };
