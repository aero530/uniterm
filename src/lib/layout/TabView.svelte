<script lang="ts">
	import { ports } from '../../stores.js';
	import Port from '../Port.svelte';
	import { Tabs, Tab, TabList, TabPanel } from '../tabs/tabs';
</script>

<Tabs>
	{#if $ports}
		<TabList>
			{#each $ports as port}
				<Tab>{port.name}</Tab>
			{/each}
		</TabList>
		
		{#each $ports as port}
			<TabPanel>
				<Port bind:port={port}/>
			</TabPanel>
		{/each}
	{/if}
</Tabs>

<style>
</style>
