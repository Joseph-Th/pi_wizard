export interface SessionCatalogEntry {
  path: string;
  id: string;
  name: string | null;
  firstMessage: string | null;
  modifiedUnixMs: number;
  previewIncomplete: boolean;
}

export interface SessionCatalogPage {
  sessions: SessionCatalogEntry[];
  candidateFiles: number;
  scannedFiles: number;
  truncated: boolean;
  nextCursor: SessionCatalogCursor | null;
  directorySource: "environment" | "settings" | "default";
}

export interface SessionCatalogCursor {
  modifiedUnixMs: number;
  path: string;
  scopeSha256: string;
  snapshotSha256: string;
}
