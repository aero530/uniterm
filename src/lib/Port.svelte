<script lang="ts">
	import PortView from './PortView.svelte';
	import PortMenu from './PortMenu.svelte';
	import type {Connection} from '../stores';
	
	export let port: Connection;
</script>

<div class="px-2 py-2 grid grid-rows-[minmax(0,1fr),minmax(8rem,12rem)]">
	<PortView bind:port={port}/>
	<PortMenu bind:port={port}/>
</div>

<style>
</style>