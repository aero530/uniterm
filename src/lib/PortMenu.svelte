<script lang="ts">
	import { invoke } from "@tauri-apps/api/tauri";
	import { save } from '@tauri-apps/api/dialog';
	import { beforeUpdate } from "svelte";
	import { serial_list, ports } from '../stores.js';
	
	import type {Connection, SerialPortType, ConfigType, LogSettingsType} from '../stores';
	
	import {sendClear, sendDisplayConfig, sendLogSettings, sendCommand, checkError} from '../messages';
	import {baudRates, flowControl, dataBits, parity, stopBits, displayMode, sendMode} from './PortMenuOptions';

	export let port: Connection;

	let inputData: string = "";
	let include_lf: boolean = false;
	let include_cr: boolean = false;
	let selected_display_size: number = 20000;
	let selected_display_mode: string = displayMode[0].value;
	let selected_send_mode: string = sendMode[0].value;

	function openLink() {
		invoke("open_connection", {id: port.id, name: port.name, baudRate: port.baud_rate, dataBits: port.data_bits, flowControl: port.flow_control, parity: port.parity, stopBits: port.stop_bits })
		.then(() => {
			ports.setIsActive(port.id, true);
		})
		.finally(() => {
			updateDisplayConfig();
		})
		.catch((error) => {
			alert(error);
			console.error(error);
			checkError(port, error);
		});
	}
	
	function closeLink() {
		invoke("close_connection", {id: port.id})
		.then((data) => {
			ports.setIsActive(port.id, false);
		})
		.catch((error) => {
			alert(error);
			console.error(error);
		});
	}
	
	function removeConnection() {
		// diconnect if connected
		if (port.is_active) {
			invoke("close_connection", {id: port.id})
			.then(() => {
				ports.removePort(port.id);
			})
			.catch((error) => {
				alert(error);
				console.error(error);
			});
		} else {
			ports.removePort(port.id);
		}
	}

	
	function updateDisplayConfig() {
		ports.setDisplayMode(port.id, selected_display_mode);
		ports.setDisplaySize(port.id, selected_display_size);
		let config : ConfigType = {
			display_mode: port.display_mode,
			max_bytes: port.max_bytes,
		}
		sendDisplayConfig(port, config);
	}

	function updateLogSettings() {
		if (port.log_path.length == 0) {
			port.is_logging = false;
			alert("Must select file first.");
		} else {
			let config : LogSettingsType = {
				enabled: port.is_logging,
				path: port.log_path,
			}
			sendLogSettings(port, config);
		}
	}

	
	function formatPortName(port: SerialPortType) {
		let output = "";
		switch (port.port_type) {
			case "UsbPort":
			if (port.product.length > 0) {
				if (!port.product.includes(port.name)) {
					output += port.name + " ";
				}
				if (!port.product.includes(port.manufacturer)) {
					output += port.manufacturer + " ";
				}
				output += port.product;
			} else {
				output = port.name + " - " + port.manufacturer;
			}
			break;
			case "PciPort" :
			output = port.name + " - PCI";
			break;
			case "BluetoothPort" :
			output = port.name + " - Bluetooth";
			break;
			case "Unknown" :
			output = port.name;
			break;
			default: 
			output = port.name;
		}
		return output;
	}
	
	// Open file save dialog and update path value in store with result
	async function getSavePath() {
		const filepathPromise = save({
			filters: [{
				name: 'Text',
				extensions: ['txt']
			}]
		});
		filepathPromise.then( filepath => {
			// filepath will be empty string if cancel button is pressed.
			// In that case, don't update the path string
			if (filepath.length > 0) {
				ports.setLogFile(port.id, filepath);
			}
		}).finally(() => updateLogSettings())
	}

	// Set the default selected port to the first port on the list
	beforeUpdate(() => {
		if (!port.name) {
			if ($serial_list.length > 0) {
				port.name = $serial_list[0];
			} else {
				port.name = ""
			}
		}
	})
	
</script>

<div class="basis-32 flex flex-col gap-1 text-sm">
	<!-- <div class="row-span-2 flex flex-col gap-1 text-sm min-h-8rem"> -->
	<div class="flex flex-row place-content-between">
		<div>
			<select
				bind:value={port.name}
				disabled={port.is_active}
				>
				{#each $serial_list as port }
				<option value={port.name}>{formatPortName(port)}</option>
				{/each}
			</select>
			<select
				bind:value={port.baud_rate}
				disabled={port.is_active}
			>
				{#each baudRates as baud}
				<option value={baud.value}>
					{baud.text}
				</option>
				{/each}
			</select>

			
			
			<select
				bind:value={port.flow_control}
				disabled={port.is_active}
			>
				{#each flowControl as mode}
				<option value={mode.value}>
					{mode.text}
				</option>
				{/each}
			</select>
		
			<select
				bind:value={port.data_bits}
				disabled={port.is_active}
			>
				{#each dataBits as mode}
				<option value={mode.value}>
					{mode.text}
				</option>
				{/each}
			</select>
			
			<select
				bind:value={port.parity}
				disabled={port.is_active}
			>
				{#each parity as mode}
				<option value={mode.value}>
					{mode.text}
				</option>
				{/each}
			</select>
			
			<select
				bind:value={port.stop_bits}
				disabled={port.is_active}
			>
				{#each stopBits as mode}
				<option value={mode.value}>
					{mode.text}
				</option>
				{/each}
			</select>

		</div>

		<div class="flex items-start gap-1">
			<button on:click={() => openLink()} disabled={port.is_active}>
				Connect
			</button>
			<button on:click={() => closeLink()} disabled={!port.is_active}>
				Disconnect
			</button>
			<button on:click={() => removeConnection()} disabled={port.is_active}>
				Remove
			</button>
		</div>
	</div>

	<div class="flex place-content-between">
		<div class="flex gap-10">
			<div>
				Display Mode 
				<select
					bind:value={selected_display_mode}
					on:change={() => updateDisplayConfig()}
					>
					{#each displayMode as mode}
					<option value={mode.value}>
						{mode.text}
					</option>
					{/each}
				</select>
			</div>
			<div>
				Scrollback Size: <input type=range min={2000} max={2000000} step={1000} bind:value={selected_display_size} on:change={() => updateDisplayConfig()}> ({selected_display_size/1000}k)
			</div>
		</div>

		<div>
			<button on:click={() => sendClear(port)} disabled={!port.is_active}>
				Clear
			</button>
		</div>
	</div>

	<div class="flex place-content-between">
		<div>
			Appending log to: 
			{#if port.log_path}
			{port.log_path}
			{/if}
			<button on:click={() => getSavePath()}>
				Set Path
			</button>			
		</div>
		<div>
			<button 
				on:click={() => {
					port.is_logging = !port.is_logging;
					updateLogSettings();
				}}
				class:bg-red-600="{port.is_logging}"
				disabled={!port.is_active}
			>
				{#if port.is_logging}Stop{:else}Start{/if} Log
			</button>
		</div>
	</div>

	<div class="flex flex-row gap-4">
		<input class="flex-1" bind:value={inputData}>
		<div>
			as 
			<select
				bind:value={selected_send_mode}
				>
				{#each sendMode as mode}
				<option value={mode.value}>
					{mode.text}
				</option>
				{/each}
			</select>
		</div>
		<div class="flex flex-row gap-2">
			<div><input type=checkbox bind:checked={include_cr}>+CR</div>
			<div><input type=checkbox bind:checked={include_lf}>+LF</div>
		</div>
		<div>
			<button on:click={() => sendCommand(port, inputData, selected_send_mode, include_cr, include_lf)} disabled={!port.is_active}>
				Send
			</button>
		</div>
	</div>

</div>

<style lang="postcss">
	button {
		@apply bg-logo-blue;
		@apply text-black;
		@apply rounded;
		@apply w-24;
		@apply text-sm;
		@apply px-1;
		@apply py-1;
		@apply text-center;
		@apply "disabled:bg-gray-300";
		@apply "disabled:text-gray-500";
	}
	select {
		@apply bg-gray-200;
		@apply border;
		@apply border-gray-500;
		@apply text-gray-700;
		@apply py-1;
		@apply px-1;
		@apply rounded;
		@apply leading-tight;
		@apply "focus:outline-none";
		@apply "focus:bg-white";
		@apply "focus:border-gray-500";
	}
</style>