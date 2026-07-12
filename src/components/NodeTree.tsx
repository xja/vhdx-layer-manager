import { statusToneFor } from "../lib/tone";
import { TreeNode, StatusLabels } from "../types";
import { Badge } from "./ui/Badge";
import { Button } from "./ui/Button";
import { Card } from "./ui/Card";

type Props = {
  data: TreeNode[];
  selectedId: string;
  onSelect: (id: string) => void;
  statusLabels: StatusLabels;
  isBusy: (cmd?: string) => boolean;
  onToggleMount: (node: TreeNode) => void;
  onToggleBoot: (node: TreeNode) => void;
  t: (key: string, options?: any) => string;
};

function displayStatus(node: TreeNode): Exclude<TreeNode["status"], "mounted"> | TreeNode["status"] {
  // Keep mounted out of the primary status badge; mount is controlled by the toggle button.
  if (node.status === "mounted") {
    return node.bcd_guid ? "normal" : "missing_bcd";
  }
  return node.status;
}

export function NodeTree({
  data,
  selectedId,
  onSelect,
  statusLabels,
  isBusy,
  onToggleMount,
  onToggleBoot,
  t,
}: Props) {
  const renderTree = (list: TreeNode[]) => {
    if (!list.length)
      return (
        <div className="rounded-xl border border-dashed border-peach-300/60 bg-white/70 px-4 py-6 text-center text-sm text-ink-700 shadow-inner shadow-peach-300/20">
          {t("tree-empty")}
        </div>
      );
    return (
      <ul className="space-y-2 border-l border-peach-300/50">
        {list.map((node) => {
          const mounted = node.status === "mounted";
          const bootReady = Boolean(node.bcd_guid || node.boot_files_ready);
          const status = displayStatus(node);
          return (
            <li key={node.id} className="pl-3">
              <div
                className={`group rounded-2xl border px-3 py-2 shadow-sm transition ${
                  selectedId === node.id
                    ? "border-peach-400 bg-white/95 shadow-peach-300/40"
                    : "border-white/70 bg-white/70 hover:-translate-y-0.5 hover:border-peach-300 hover:bg-white"
                }`}
                onClick={() => onSelect(node.id)}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 space-y-2">
                    <p className="truncate text-base font-semibold text-ink-900">{node.name}</p>
                    <div className="flex flex-wrap items-center gap-2 text-xs">
                      <Badge tone={statusToneFor(status as any)} className="px-2 py-1 text-[11px]">
                        {statusLabels[status as keyof StatusLabels] || statusLabels.normal}
                      </Badge>
                      <Button
                        variant={mounted ? "danger" : "secondary"}
                        className="px-2 py-1 text-[11px]"
                        loading={isBusy("attach_vhd") || isBusy("detach_vhd")}
                        disabled={isBusy()}
                        onClick={(e) => {
                          e.stopPropagation();
                          onToggleMount(node);
                        }}
                      >
                        {mounted ? t("mount-toggle-off") : t("mount-toggle-on")}
                      </Button>
                      <Button
                        variant={bootReady ? "danger" : "secondary"}
                        className="px-2 py-1 text-[11px]"
                        loading={isBusy("add_bcd_entry") || isBusy("delete_bcd")}
                        disabled={isBusy()}
                        onClick={(e) => {
                          e.stopPropagation();
                          onToggleBoot(node);
                        }}
                      >
                        {bootReady ? t("boot-toggle-off") : t("boot-toggle-on")}
                      </Button>
                    </div>
                  </div>
                  <span className="rounded-full bg-peach-50 px-2 py-1 text-[11px] font-mono text-ink-700 shadow-inner shadow-peach-300/30">
                    {node.children.length}
                  </span>
                </div>
              </div>
              {node.children.length > 0 && <div className="ml-3 mt-2">{renderTree(node.children)}</div>}
            </li>
          );
        })}
      </ul>
    );
  };

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden p-4">
      <div className="mb-3">
        <h3 className="text-sm font-semibold text-ink-900">{t("node-tree-title")}</h3>
        <p className="text-xs text-ink-700">{t("node-tree-tip")}</p>
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">{renderTree(data)}</div>
    </Card>
  );
}
