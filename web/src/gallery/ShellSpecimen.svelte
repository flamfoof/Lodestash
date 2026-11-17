<script lang="ts">
  import Shell from "../app/Shell.svelte";
  import type { ShellData } from "../app/shell";
  import { EmptyState } from "../ui";

  /*
   * The shell with the prototype's library-state data (its 19 assets: 5 unidentified, 9
   * identified, 5 confirmed; four source libraries; one duplicate group). `gallery.html#shell`
   * renders only this, full-window, for the gate to measure the frame at the contract viewport.
   */
  let data = $state<ShellData>({
    route: "library",
    badges: { identify: 5, duplicates: 1, trash: 0 },
    mode: "tags",
    query: "",
    category: "all",
    health: { identified: 14, total: 19 },
    project: { name: "MyRPG", engine: "Unity 6", color: "var(--ls-success)" },
    device: "RTX 4070",
    status: { value: "all", counts: { all: 19, unidentified: 5, identified: 9, confirmed: 5 } },
    libraries: {
      value: "all",
      options: [
        { id: "all", label: "All libraries", count: 19 },
        { id: "synty", label: "Synty", count: 3 },
        { id: "human3", label: "Human3", count: 2 },
        { id: "asset-store", label: "Asset Store", count: 8 },
        { id: "humble", label: "Humble", count: 6 },
      ],
    },
  });
</script>

<Shell
  {data}
  onnavigate={(id) => (data.route = id)}
  onmode={(m) => (data.mode = m)}
  onquery={(q) => (data.query = q)}
  oncategory={(c) => (data.category = c)}
  onstatus={(s) => (data.status.value = s)}
  onlibrary={(l) => (data.libraries.value = l)}
>
  <EmptyState icon="nav-library" title="The frame, with fixture data" line="Screens arrive in plan P7; the shell's data arrives from /v1 in P6b." />
</Shell>
