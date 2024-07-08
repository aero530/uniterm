<script>
	import { getContext } from 'svelte';
	import { TABS } from './Tabs.svelte';

	const tab = {};
	const { registerTab, selectTab, selectedTab } = getContext(TABS);

	registerTab(tab);
</script> 

<style lang="postcss">
	.selected {
		border-color: rgb(23 53 100);
        background-color: rgb(131, 159, 216);
		color: rgb(0 0 0);
	}
</style>

<button 
	class="px-2 self-end m-0 border-0 border-b-4 border-white-500 bg-gray-100 text-gray-400"
	class:selected="{$selectedTab === tab}"
	on:click="{() => selectTab(tab)}"
>
	<slot></slot>
</button>