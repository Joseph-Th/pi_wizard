export interface DesktopProjectRecord {
  id: string;
  canonicalRoot: string;
  status: "present" | "missing" | "changed" | "unverifiable";
  detail: string | null;
}
