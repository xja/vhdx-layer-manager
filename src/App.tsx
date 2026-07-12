import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { NodeDetail } from "./components/NodeDetail";
import { NodeTree } from "./components/NodeTree";
import { WorkspaceGate } from "./components/WorkspaceGate";
import { WorkspaceLogPanel } from "./components/WorkspaceLogPanel";
import { Node, RecentWorkspace, Settings, StatusLabels, TreeNode, WimImageInfo, WorkspaceLogEntry } from "./types";
import { Badge } from "./components/ui/Badge";
import { Button } from "./components/ui/Button";
import { Card } from "./components/ui/Card";
import { useCommandRunner } from "./hooks/useCommandRunner";

const QUIET_LOG_COMMANDS = new Set(["list_nodes", "list_recent_workspaces", "get_settings"]);

function App() {
  const { t, i18n } = useTranslation();
  const [rootPath, setRootPath] = useState("");
  const [admin, setAdmin] = useState<boolean | null>(null);
  const [message, setMessage] = useState("");
  const [status, setStatus] = useState<"idle" | "initialized" | "error">("idle");
  const [workspaceReady, setWorkspaceReady] = useState(false);
  const [nodes, setNodes] = useState<Node[]>([]);
  const [recents, setRecents] = useState<RecentWorkspace[]>([]);
  const [baseName, setBaseName] = useState("base");
  const [baseSize, setBaseSize] = useState(60);
  const [baseDesc, setBaseDesc] = useState("");
  const [wimPath, setWimPath] = useState("");
  const [wimIndex, setWimIndex] = useState(1);
  const [wimImages, setWimImages] = useState<WimImageInfo[]>([]);
  const [diffName, setDiffName] = useState("child");
  const [diffDesc, setDiffDesc] = useState("");
  const [bcdName, setBcdName] = useState("");
  const [selectedNode, setSelectedNode] = useState("");
  const [logs, setLogs] = useState<WorkspaceLogEntry[]>([]);
  const [logFilter, setLogFilter] = useState<"all" | "error" | "op">("all");
  const previewMode = import.meta.env.VITE_UI_PREVIEW === "1";
  const nodesRef = useRef<Node[]>([]);
  const rootPathRef = useRef("");
  const localeRef = useRef(i18n.language);

  const appendLog = useCallback(
    (entry: Omit<WorkspaceLogEntry, "id" | "ts"> & { id?: string; ts?: string }) => {
      setLogs((prev) => [
        {
          id: entry.id || `${Date.now()}-${Math.random().toString(16).slice(2)}`,
          ts: entry.ts || new Date().toISOString(),
          level: entry.level,
          source: entry.source,
          title: entry.title,
          detail: entry.detail,
          nodeId: entry.nodeId,
          command: entry.command,
        },
        ...prev,
      ].slice(0, 300));
    },
    [],
  );

  const { run: runCommandRaw, isBusy } = useCommandRunner({ setStatus, setMessage, t });

  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  useEffect(() => {
    rootPathRef.current = rootPath;
  }, [rootPath]);

  useEffect(() => {
    localeRef.current = i18n.language;
  }, [i18n.language]);

  const runCommand = useCallback(
    async <T,>(cmd: string, args?: Record<string, unknown>) => {
      const nodeId =
        typeof args?.nodeId === "string"
          ? args.nodeId
          : typeof args?.parentId === "string"
            ? args.parentId
            : undefined;
      const shouldLog = !QUIET_LOG_COMMANDS.has(cmd);

      if (shouldLog) {
        appendLog({
          level: "info",
          source: "runtime",
          title: t("log-level.info"),
          detail: cmd,
          command: cmd,
          nodeId,
        });
      }

      try {
        let result: T;
        if (previewMode) {
          // Local UI preview without Tauri backend.
          if (cmd === "list_nodes" || cmd === "scan_workspace") {
            result = nodesRef.current as T;
          } else if (cmd === "attach_vhd" || cmd === "detach_vhd") {
            const id = String(args?.nodeId || "");
            setNodes((prev) =>
              prev.map((n) => {
                if (n.id !== id) return n;
                return {
                  ...n,
                  status: cmd === "attach_vhd" ? "mounted" : n.bcd_guid ? "normal" : "missing_bcd",
                };
              }),
            );
            result = { id } as T;
          } else if (cmd === "add_bcd_entry") {
            const id = String(args?.nodeId || "");
            setNodes((prev) =>
              prev.map((n) => {
                if (n.id !== id) return n;
                return {
                  ...n,
                  bcd_guid: n.bcd_guid || `{preview-${n.id}}`,
                  boot_files_ready: true,
                  status: n.status === "mounted" ? "mounted" : "normal",
                };
              }),
            );
            result = `{preview-${id}}` as T;
          } else if (cmd === "delete_bcd") {
            const id = String(args?.nodeId || "");
            setNodes((prev) =>
              prev.map((n) =>
                n.id === id
                  ? {
                      ...n,
                      bcd_guid: null,
                      boot_files_ready: false,
                      status: n.status === "mounted" ? "mounted" : "missing_bcd",
                    }
                  : n,
              ),
            );
            result = undefined as T;
          } else if (cmd === "list_recent_workspaces") {
            result = [] as T;
          } else if (cmd === "get_settings") {
            result = {
              root_path: rootPathRef.current || "E:\\test-vhdx",
              locale: localeRef.current,
              seq_counter: 3,
            } as T;
          } else {
            result = undefined as T;
          }
        } else {
          result = await runCommandRaw<T>(cmd, args);
        }

        if (shouldLog) {
          appendLog({
            level: "success",
            source: "op",
            title: cmd,
            detail: previewMode ? "preview mock ok" : undefined,
            command: cmd,
            nodeId,
          });
        }
        return result;
      } catch (err) {
        appendLog({
          level: "error",
          source: "error",
          title: cmd,
          detail: String(err),
          command: cmd,
          nodeId,
        });
        throw err;
      }
    },
    [appendLog, runCommandRaw, t, previewMode],
  );

  const statusLabels = useMemo<StatusLabels>(
    () => ({
      normal: t("node-status.normal"),
      missing_file: t("node-status.missing-file"),
      missing_parent: t("node-status.missing-parent"),
      missing_bcd: t("node-status.missing-bcd"),
      mounted: t("node-status.mounted"),
      error: t("node-status.error"),
    }),
    [t],
  );

  const adminLabel = useMemo(() => {
    if (admin === null) return "...";
    return admin ? t("admin-yes") : t("admin-no");
  }, [admin, t]);

  const refreshNodes = useCallback(async () => {
    if (!workspaceReady) return;
    try {
      const list = await runCommand<Node[]>("list_nodes");
      setNodes(list);
    } catch {
      // errors are handled in runCommand
    }
  }, [workspaceReady, runCommand]);

  const syncNodes = useCallback(async () => {
    try {
      const list = await runCommand<Node[]>("scan_workspace");
      setNodes(list);
      return true;
    } catch {
      // errors are handled in runCommand
      return false;
    }
  }, [runCommand]);

  const refreshRecents = useCallback(async () => {
    try {
      const list = await runCommand<RecentWorkspace[]>("list_recent_workspaces");
      setRecents(list);
    } catch {
      // handled in runCommand
    }
  }, [runCommand]);

  useEffect(() => {
    if (previewMode) {
      const now = new Date().toISOString();
      setAdmin(true);
      setRootPath("E:\\test-vhdx");
      setStatus("initialized");
      setWorkspaceReady(true);
      setMessage(t("preview-banner"));
      setNodes([
        {
          id: "base-1",
          parent_id: null,
          name: "base",
          path: "E:\\test-vhdx\\disks\\0002-base.vhdx",
          bcd_guid: "{11111111-1111-1111-1111-111111111111}",
          desc: "preview base",
          created_at: now,
          status: "normal",
          boot_files_ready: true,
        },
        {
          id: "child-1",
          parent_id: "base-1",
          name: "child",
          path: "E:\\test-vhdx\\disks\\0003-base-child.vhdx",
          bcd_guid: null,
          desc: "preview child",
          created_at: now,
          status: "mounted",
          boot_files_ready: false,
        },
      ]);
      setSelectedNode("base-1");
      appendLog({
        level: "info",
        source: "ui",
        title: t("preview-banner"),
        detail: "VITE_UI_PREVIEW=1",
      });
      return;
    }

    const bootstrap = async () => {
      try {
        const isAdmin = await invoke<boolean>("check_admin");
        setAdmin(isAdmin);
      } catch (err) {
        setAdmin(false);
      }

      try {
        await refreshRecents();
      } catch {
        // handled in runCommand
      }

      try {
        const settings = await runCommand<Settings | null>("get_settings");
        if (settings) {
          setRootPath(settings.root_path);
          setStatus("initialized");
          setMessage(t("status-initialized", { path: settings.root_path }));
          i18n.changeLanguage(settings.locale || "zh-CN");
          setWorkspaceReady(true);
          await syncNodes();
        } else {
          setMessage(t("status-uninitialized"));
          setWorkspaceReady(false);
        }
      } catch {
        // handled in runCommand
      }
    };
    bootstrap();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!workspaceReady || !nodes.length) {
      setSelectedNode("");
      return;
    }
    if (!selectedNode) {
      setSelectedNode(nodes[0].id);
    } else if (!nodes.some((n) => n.id === selectedNode)) {
      setSelectedNode(nodes[0].id);
    }
  }, [workspaceReady, nodes, selectedNode]);

  useEffect(() => {
    if (!workspaceReady || previewMode) return;
    refreshNodes();
  }, [workspaceReady, refreshNodes, previewMode]);

  const treeData = useMemo<TreeNode[]>(() => {
    const map = new Map<string, TreeNode>();
    nodes.forEach((n) => map.set(n.id, { ...n, children: [] }));
    const roots: TreeNode[] = [];

    map.forEach((node) => {
      const parentId = node.parent_id || "";
      if (parentId && map.has(parentId)) {
        map.get(parentId)!.children.push(node);
      } else {
        roots.push(node);
      }
    });

    const sortRecursively = (list: TreeNode[]) => {
      list.sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
      list.forEach((child) => sortRecursively(child.children));
    };
    sortRecursively(roots);
    return roots;
  }, [nodes]);

  const selectedDetail = useMemo(
    () => nodes.find((n) => n.id === selectedNode) || null,
    [nodes, selectedNode],
  );
  const parentNode = useMemo(
    () => nodes.find((n) => n.id === selectedDetail?.parent_id) || null,
    [nodes, selectedDetail],
  );

  useEffect(() => {
    if (selectedDetail) {
      setDiffName(`${selectedDetail.name}-child`);
      setDiffDesc("");
      setBcdName(selectedDetail.name);
    }
  }, [selectedDetail?.id]);

  const handleLocaleChange = (lng: string) => {
    i18n.changeLanguage(lng);
  };

  const handleListWim = useCallback(async () => {
    try {
      const res = await runCommand<WimImageInfo[]>("list_wim_images", { imagePath: wimPath });
      setWimImages(res);
      setMessage(t("message-wim-loaded", { count: res.length }));
    } catch {
      // handled in runCommand
    }
  }, [runCommand, t, wimPath]);


  const isDriveRootPath = useCallback((value: string) => {
    let normalized = value.trim().replace(/\//g, "\\");
    if (normalized.startsWith("\\\\?\\")) {
      normalized = normalized.slice(4);
    }
    normalized = normalized.replace(/[\\/]+$/, "");
    return /^[A-Za-z]:$/.test(normalized);
  }, []);

  const validateRootPath = useCallback(
    (value: string) => {
      const targetPath = value.trim();
      if (!targetPath) {
        return t("error-empty-root");
      }
      if (isDriveRootPath(targetPath)) {
        return t("error-drive-root");
      }
      return null;
    },
    [isDriveRootPath, t],
  );

  const handleOpenExisting = useCallback(
    async (pathOverride?: unknown) => {
      const rawPath = typeof pathOverride === "string" ? pathOverride : rootPath;
      const targetPath = (rawPath || "").trim();
      setRootPath(targetPath);
      const validationError = validateRootPath(targetPath);
      if (validationError) {
        setMessage(t("status-error", { msg: validationError }));
        setStatus("error");
        return;
      }
      try {
        const result = await runCommand<{ settings: Settings }>("init_root", {
          rootPath: targetPath,
          locale: i18n.language,
        });
        setStatus("initialized");
        setWorkspaceReady(true);
        setMessage(t("status-initialized", { path: result.settings.root_path }));
        await syncNodes();
      } catch {
        // handled in runCommand
      } finally {
        await refreshRecents();
      }
    },
    [rootPath, runCommand, i18n.language, t, syncNodes, refreshRecents, validateRootPath],
  );

  const handleCreateWorkspace = useCallback(async () => {
    const targetPath = rootPath.trim();
    setRootPath(targetPath);
    const validationError = validateRootPath(targetPath);
    if (validationError) {
      setMessage(t("status-error", { msg: validationError }));
      setStatus("error");
      return;
    }
    let workspaceOpened = false;
    try {
      await runCommand<{ settings: Settings }>("init_root", {
        rootPath: targetPath,
        locale: i18n.language,
      });
      setStatus("initialized");
      setWorkspaceReady(true);
      workspaceOpened = true;
      const res = await runCommand<{ node: Node }>("create_base_vhd", {
        name: baseName,
        desc: baseDesc || null,
        wimFile: wimPath,
        wimIndex,
        sizeGb: baseSize,
      });
      setMessage(t("message-base-created", { name: res.node.name }));
    } catch {
      // handled in runCommand
    } finally {
      if (workspaceOpened) {
        await syncNodes();
      }
      await refreshRecents();
    }
  }, [rootPath, runCommand, i18n.language, baseName, baseDesc, wimPath, wimIndex, baseSize, t, syncNodes, refreshRecents, validateRootPath]);


  const handleToggleMount = useCallback(
    async (node: Node) => {
      setSelectedNode(node.id);
      try {
        if (node.status === "mounted") {
          await runCommand<Node>("detach_vhd", { nodeId: node.id });
          setMessage(t("message-detached", { name: node.name }));
        } else {
          await runCommand<Node>("attach_vhd", { nodeId: node.id });
          setMessage(t("message-attached", { name: node.name }));
        }
      } catch {
        // handled
      } finally {
        if (!previewMode) {
          await syncNodes();
        }
      }
    },
    [runCommand, syncNodes, t, previewMode],
  );

  const handleToggleBoot = useCallback(
    async (node: Node) => {
      setSelectedNode(node.id);
      try {
        if (node.bcd_guid || node.boot_files_ready) {
          await runCommand("delete_bcd", { nodeId: node.id });
          setMessage(t("message-deleted-bcd"));
        } else {
          const guid = await runCommand<string | null>("add_bcd_entry", {
            nodeId: node.id,
            description: node.name,
          });
          setMessage(t("message-repaired-bcd", { guid: guid ?? t("message-no-guid") }));
        }
      } catch {
        // handled
      } finally {
        if (!previewMode) {
          await syncNodes();
        }
      }
    },
    [runCommand, syncNodes, t, previewMode],
  );

  const handleCreateDiff = useCallback(async () => {
    if (!selectedNode) return;
    try {
      const res = await runCommand<{ node: Node }>("create_diff_vhd", {
        parentId: selectedNode,
        name: diffName,
        desc: diffDesc || null,
      });
      setMessage(t("message-diff-created", { name: res.node.name }));
    } catch {
      // handled in runCommand
    } finally {
      await syncNodes();
    }
  }, [selectedNode, runCommand, diffName, diffDesc, t, syncNodes]);

  const handleCheck = useCallback(async () => {
    if (await syncNodes()) {
      setMessage(t("message-checked"));
    }
  }, [syncNodes, t]);

  const handleBootReboot = useCallback(async () => {
    if (!selectedNode) return;
    try {
      await runCommand("set_bootsequence_and_reboot", { nodeId: selectedNode });
      setMessage(t("message-boot-set"));
    } catch {
      // handled in runCommand
    }
  }, [selectedNode, runCommand, t]);

  const handleStartVm = useCallback(async () => {
    if (!selectedNode) return;
    try {
      const res = await runCommand<{ vm_name: string }>("start_vm", { nodeId: selectedNode });
      const label = res?.vm_name || selectedDetail?.name || selectedNode;
      setMessage(t("message-vm-started", { name: label }));
    } catch {
      // handled in runCommand
    }
  }, [selectedNode, selectedDetail?.name, runCommand, t]);

  const handleDelete = useCallback(async () => {
    if (!selectedNode) return;
    try {
      await runCommand("delete_subtree", { nodeId: selectedNode });
      setMessage(t("message-deleted"));
    } catch {
      // handled in runCommand
    } finally {
      await syncNodes();
    }
  }, [selectedNode, runCommand, syncNodes, t]);

  const handleAddBcd = useCallback(async () => {
    if (!selectedNode) return;
    try {
      const guid = await runCommand<string | null>("add_bcd_entry", {
        nodeId: selectedNode,
        description: bcdName || null,
      });
      setMessage(t("message-repaired-bcd", { guid: guid ?? t("message-no-guid") }));
    } catch {
      // handled in runCommand
    } finally {
      await syncNodes();
    }
  }, [selectedNode, runCommand, syncNodes, bcdName, t]);

  const handleUpdateBcdDesc = useCallback(async () => {
    if (!selectedNode) return;
    try {
      await runCommand("update_bcd_description", { nodeId: selectedNode, description: bcdName });
      setMessage(t("message-updated-bcd"));
      await syncNodes();
    } catch {
      // handled in runCommand
    }
  }, [selectedNode, runCommand, syncNodes, bcdName, t]);

  const handleDeleteBcd = useCallback(async () => {
    if (!selectedNode) return;
    try {
      await runCommand("delete_bcd", { nodeId: selectedNode });
      setMessage(t("message-deleted-bcd"));
      await syncNodes();
    } catch {
      // handled in runCommand
    }
  }, [selectedNode, runCommand, syncNodes, t]);

  const handleCloseWorkspace = useCallback(() => {
    setWorkspaceReady(false);
    setNodes([]);
    setSelectedNode("");
    setWimImages([]);
    setWimPath("");
    setWimIndex(1);
    setBaseName("base");
    setBaseSize(60);
    setBaseDesc("");
    setDiffName("child");
    setDiffDesc("");
    setStatus("idle");
    setMessage(t("message-workspace-closed"));
    setRootPath("");
    refreshRecents().catch(() => {});
  }, [t, refreshRecents]);

  const handleUseRecent = useCallback(
    async (path: string) => {
      await handleOpenExisting(path);
    },
    [handleOpenExisting],
  );

  const handleRemoveRecent = useCallback(
    async (path: string) => {
      try {
        await runCommand("remove_recent_workspace", { path });
        await refreshRecents();
      } catch {
        // handled in runCommand
      }
    },
    [runCommand, refreshRecents],
  );

  const handleClearRecents = useCallback(async () => {
    try {
      await runCommand("clear_recent_workspaces");
      await refreshRecents();
    } catch {
      // handled in runCommand
    }
  }, [runCommand, refreshRecents]);

  return (
    <div className="h-screen overflow-hidden bg-gradient-to-br from-peach-50 via-peach-200/50 to-peach-400/40 font-sans text-ink-900">
      <main className="mx-auto flex h-full max-w-6xl flex-col gap-4 px-3 py-4 sm:px-5 sm:py-5 lg:px-6">
        <Card className="p-5 shadow-lg shadow-peach-300/30">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.28em] text-peach-400">{t("subtitle")}</p>
              <h1 className="text-3xl font-bold leading-tight sm:text-4xl">{t("title")}</h1>
            </div>
            <div className="flex items-center gap-3 rounded-full border border-peach-200/80 bg-peach-50/80 px-3 py-2 shadow-inner shadow-peach-400/25">
              <label
                htmlFor="locale"
                className="text-xs font-semibold uppercase tracking-wide text-ink-700"
              >
                {t("locale-label")}
              </label>
              <select
                id="locale"
                value={i18n.language}
                onChange={(e) => handleLocaleChange(e.target.value)}
                className="rounded-full border border-peach-200 bg-white/90 px-3 py-2 text-sm font-semibold text-ink-900 shadow-sm shadow-peach-200/50 focus:border-peach-300 focus:outline-none focus:ring-2 focus:ring-peach-300/60"
              >
                <option value="zh-CN">{t("locale-zh")}</option>
                <option value="en">{t("locale-en")}</option>
              </select>
            </div>
          </div>
        </Card>

        {!workspaceReady ? (
          <div className="flex-1 overflow-auto">
            <WorkspaceGate
              rootPath={rootPath}
              setRootPath={setRootPath}
              wimPath={wimPath}
              setWimPath={setWimPath}
              wimIndex={wimIndex}
              setWimIndex={setWimIndex}
              baseSize={baseSize}
              setBaseSize={setBaseSize}
              baseName={baseName}
              setBaseName={setBaseName}
              baseDesc={baseDesc}
              setBaseDesc={setBaseDesc}
              wimImages={wimImages}
              recents={recents}
              onListWim={handleListWim}
              onOpenExisting={handleOpenExisting}
              onUseRecent={handleUseRecent}
              onRemoveRecent={handleRemoveRecent}
              onClearRecents={handleClearRecents}
              onRefreshRecents={refreshRecents}
              onCreateWorkspace={handleCreateWorkspace}
              status={status}
              message={message}
              admin={admin}
              adminLabel={adminLabel}
              isBusy={isBusy}
              t={t}
            />
          </div>
        ) : (
          <section className="flex min-h-0 flex-1 flex-col gap-4 overflow-hidden">
            <Card className="flex flex-wrap items-center justify-between gap-3 p-4 shadow-md shadow-peach-300/25">
              <div className="flex flex-wrap items-center gap-3">
                {previewMode ? (
                  <Badge tone="info" className="max-w-xl px-3 py-1">
                    {t("preview-banner")}
                  </Badge>
                ) : null}
                <Badge tone={admin ? "positive" : "warn"} className="px-3 py-1">
                  {adminLabel}
                </Badge>
                <span className="truncate font-mono text-sm text-ink-700">{rootPath}</span>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="secondary"
                  onClick={refreshNodes}
                  disabled={isBusy("list_nodes")}
                  loading={isBusy("list_nodes")}
                >
                  {t("refresh-button")}
                </Button>
                <Button
                  variant="secondary"
                  onClick={handleCheck}
                  disabled={isBusy("scan_workspace")}
                  loading={isBusy("scan_workspace")}
                >
                  {t("check-button")}
                </Button>
                <Button
                  variant="danger"
                  onClick={handleCloseWorkspace}
                  disabled={isBusy()}
                  loading={isBusy()}
                >
                  {t("close-workspace")}
                </Button>
                <Badge
                  tone={status === "initialized" ? "positive" : status === "error" ? "danger" : "neutral"}
                  className="max-w-xs truncate px-3 py-2"
                >
                  {message}
                </Badge>
              </div>
            </Card>

            <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-hidden lg:grid-cols-[360px_minmax(0,1fr)]">
              <div className="grid min-h-0 grid-rows-[minmax(0,1.2fr)_minmax(0,0.9fr)] gap-4 overflow-hidden">
                <NodeTree
                  data={treeData}
                  selectedId={selectedNode}
                  onSelect={(id) => setSelectedNode(id)}
                  statusLabels={statusLabels}
                  isBusy={isBusy}
                  onToggleMount={handleToggleMount}
                  onToggleBoot={handleToggleBoot}
                  t={t}
                />
                <WorkspaceLogPanel
                  logs={logs}
                  filter={logFilter}
                  onFilterChange={setLogFilter}
                  onClear={() => setLogs([])}
                  onFocusNode={(id) => setSelectedNode(id)}
                  t={t}
                />
              </div>
              <NodeDetail
                selected={selectedDetail}
                parentNode={parentNode}
                statusLabels={statusLabels}
                diffName={diffName}
                diffDesc={diffDesc}
                setDiffName={setDiffName}
                setDiffDesc={setDiffDesc}
                bcdName={bcdName}
                setBcdName={setBcdName}
                onAddBcd={handleAddBcd}
                onUpdateBcd={handleUpdateBcdDesc}
                onCreateDiff={handleCreateDiff}
                onBoot={handleBootReboot}
                onStartVm={handleStartVm}
                onDeleteBcd={handleDeleteBcd}
                onDelete={handleDelete}
                isBusy={isBusy}
                t={t}
              />
            </div>
          </section>
        )}
      </main>
    </div>
  );
}

export default App;
