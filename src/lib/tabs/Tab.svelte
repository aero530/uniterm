<script>
	import { getContext } from 'svelte';
	import { TABS } from './Tabs.svelte';

	const tab = {};
	const { registerTab, selectTab, selectedTab } = getContext(TABS);

	registerTab(tab);
</script>

<style lang="postcss">
	button {
        @apply m-0;
        @apply border-0;
        @apply border-b-2;
        @apply border-white-500;
        @apply bg-gray-100;
        @apply text-gray-400;
	}
	
	.selected {
        @apply border-logo-blue;
        @apply bg-gray-200;
        @apply text-black;
	}
</style>

<button class="px-2 self-end" class:selected="{$selectedTab === tab}" on:click="{() => selectTab(tab)}">
	<slot></slot>
</button>