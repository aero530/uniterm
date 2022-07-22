<script lang="ts">
	import PortView from './PortView.svelte';
	import PortMenu from './PortMenu.svelte';
	import type {Connection} from '../stores';
	
	export let port: Connection;
</script>


<!-- <div class="row-span-11 px-2 py-2 grid grid-rows-12"> -->
<div class="row-span-11 px-2 py-2 flex flex-col">
	<PortView bind:port={port}/>
	<PortMenu bind:port={port}/>
</div>

<style>
</style>

