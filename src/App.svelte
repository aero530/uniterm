<script lang="ts">
	import { listen, emit } from "@tauri-apps/api/event";
	import { onMount, onDestroy } from "svelte";
	import { invoke } from "@tauri-apps/api/tauri";
	import Icon from '@iconify/svelte';
	
	
	import { serial_list, ports } from './stores.js';
	
	import Tooltip from './lib/Tooltip.svelte';
	
	import LayoutSingle from './lib/layout/Single.svelte';
	import LayoutDouble from './lib/layout/Double.svelte';
	import LayoutTripple from './lib/layout/Tripple.svelte';
	import LayoutQuad from './lib/layout/Quad.svelte';
	import type {SerialPortType} from './stores';
	
	let unlisten_serial;
	
	const pages = [
	{text: 'Single', value: LayoutSingle, to: 'Single'},
	{text: 'Double', value: LayoutDouble, to: 'Double'},
	{text: 'Tripple', value: LayoutTripple, to: 'Tripple'},
	{text: 'Quad', value: LayoutQuad, to: 'Quad'},
	];
	let selected = pages[0];
	
	
	function refreshSerialList() {
		invoke('get_port_list')
		.then((data: [SerialPortType]) => {
			serial_list.set(data);
		});
	}
	
	onMount(async () => {
		
		refreshSerialList();

		unlisten_serial = await listen("serial", event => {
			switch (event.payload.command) {
				case 'rx_bytes' :
				console.log(event.payload.data)
				console.log(event);
				ports.updateRxBuffer(event.payload.id, event.payload.data);
				break;
				case 'rx_buffer' :
				console.log(event.payload.data)
				ports.setRxBuffer(event.payload.id, event.payload.data);
				break;
				case 'error' :
				alert(event.payload.data);
				console.error(event.payload.data);
				console.log(event.payload);
				break;
				case 'close' :
				alert(event.payload.data);
				console.error(event.payload.data);
				ports.setIsActive(event.payload.id, false);
				break;				
				default : 
				break;
			}
		})
	})
	
	onDestroy(() => {
		if (unlisten_serial) {
			unlisten_serial()
		}
	})
	
	function addConnection() {
		if ($serial_list.length > 0) {
			ports.addPort({name: $serial_list[0].name, baud_rate: 115200});
		} else {
			ports.addPort({name: "", baud_rate: 115200});
		}
		
	}
	
</script>

<div>
	<div class="top-0 left-0 w-12 h-screen fixed bg-gray-300 text-center">
		<grid class="grid gap-2 grid-cols-1 mt-5 px-2 w-full place-items-center">
			<Tooltip tip="Refresh serial port list">
				<button on:click={refreshSerialList} class="hover:bg-gray-200 text-black rounded w-8 px-1 py-1 cursor-pointer" >
					<Icon icon="system-uicons:refresh" width="100%" height="100%"/>
				</button>
			</Tooltip>
			
			<Tooltip tip="Add port tab">
				<button on:click={addConnection} class="hover:bg-gray-200  text-black rounded w-8 px-1 py-1 cursor-pointer">
					<Icon icon="system-uicons:button-add" width="100%" height="100%"/>
				</button>
			</Tooltip>
			
			<div class="h-10"></div>
			
			<Tooltip tip="Single View">
				<button on:click={() => {selected = pages[0]}} class="hover:bg-gray-200  text-black rounded w-8 px-1 py-1 cursor-pointer">
					<Icon icon="system-uicons:checkbox-empty" width="100%" height="100%"/>
				</button>
			</Tooltip>
			<Tooltip tip="Split View">
				<button on:click={() => {selected = pages[1]}} class="hover:bg-gray-200  text-black rounded w-8 px-1 py-1 cursor-pointer">
					<Icon icon="system-uicons:split" width="100%" height="100%"/>
				</button>
			</Tooltip>
			<Tooltip tip="Three View">
				<button on:click={() => {selected = pages[2]}} class="hover:bg-gray-200  text-black rounded w-8 px-1 py-1 cursor-pointer">
					<Icon icon="system-uicons:split-three" width="100%" height="100%"/>
				</button>
			</Tooltip>
			<Tooltip tip="Grid View">
				<button on:click={() => {selected = pages[3]}} class="hover:bg-gray-200  text-black rounded w-8 px-1 py-1 cursor-pointer">
					<Icon icon="system-uicons:grid" width="100%" height="100%"/>
				</button>
			</Tooltip>
		</grid>
	</div>
	
	<div class="top-0 right-0 pl-12 bg-gray-100 h-screen">
		<svelte:component this={selected.value}/>
	</div>
</div>
	
<style lang="postcss">
	:root {
		font-family: 'Fira Code VF', monospace, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen,
		Ubuntu, Cantarell, 'Open Sans', 'Helvetica Neue', sans-serif;
	}
	
	:global(.splitpanes__pane) {
		/* @apply flex;
		@apply flex-row; */
    }
</style>