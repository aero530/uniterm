<script lang="ts">
	import type {Connection} from '../stores';
	import { beforeUpdate, afterUpdate } from 'svelte';
	import Convert from 'ansi-to-html';
	var convert = new Convert({newline: true});

	import {sendAsciiCode} from '../messages';
	import {asciiCodes} from './asciiCodes';

	export let port: Connection;

	let box;
	let autoscroll;
	let is_focused = false;

	function handleKeydown(event) {
		if (is_focused) {
			let byte: number;

			byte = asciiCodes[event.key];

			if (byte!==undefined) {
				sendAsciiCode(port, byte);
				if (byte===13) { // If this was a CR send a LF as well.
					sendAsciiCode(port, 10);
				}
			}
		}
	}

	function changeFocus(state: boolean) {
		is_focused = state;
	}

	beforeUpdate(() => {
		autoscroll = box && (box.offsetHeight + box.scrollTop) > (box.scrollHeight - 20);
	});

	afterUpdate(() => {
		if (autoscroll) box.scrollTo(0, box.scrollHeight);
	});

</script>

<!-- 
flex-wrap : allows subsequent divs to wrap (this allows the break div fom ansi_to_html to work)
items-start : helps clean up spacing for divs that have background color
basis-full : forces the div to be as tall as possible
overflow-scroll : allows scrolling within the div
whitespace-pre : Formats white space as it is in the string (spaces and line breaks show up)	
-->

<div 
	tabindex="0"
	bind:this={box}
	on:keydown|preventDefault={handleKeydown}
	on:focus={() => changeFocus(true)}
	on:blur={() => changeFocus(false)}
	class="flex flex-wrap items-start min-h-8rem border text-white overflow-scroll mb-2"
	class:bg-black="{!is_focused}"
	class:bg-gray-800="{is_focused}"
	class:border-4="{is_focused}"
	class:border-red-500="{is_focused}"
	class:whitespace-pre="{port.display_mode=="Ascii" || port.display_mode=="Ansi"}"
>
{@html port.rx_buffer}
</div>
<!-- The code block inside the text area must not have any whitespace before it or it will display 
in the UI.  So this code block can not be indented for readability in the source. -->

<style>
</style>