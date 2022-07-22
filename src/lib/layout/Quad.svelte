<script lang="ts">
	import { Pane, Splitpanes } from 'svelte-splitpanes';
	import TabView from './TabView.svelte';
</script>

<Splitpanes class="default-theme">
	<Pane>
		<Splitpanes class="default-theme" horizontal="{true}">
			<Pane><TabView/></Pane>
			<Pane><TabView/></Pane>
		</Splitpanes>
	</Pane>
	<Pane>
		<Splitpanes class="default-theme" horizontal="{true}">
			<Pane><TabView/></Pane>
			<Pane><TabView/></Pane>
		</Splitpanes>
	</Pane>
</Splitpanes>

<style>
</style>
