<script lang="ts">
    import { Pane, Splitpanes } from 'svelte-splitpanes';
    import TabView from './TabView.svelte';
</script>

<Splitpanes class="default-theme">
	<Pane><TabView/></Pane>
    <Pane><TabView/></Pane>
</Splitpanes>
  
<style>
</style>
