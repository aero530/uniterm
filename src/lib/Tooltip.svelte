<script>
	export let tip = "";
	export let active = false;
</script>

<style>
	.tooltip {
		position: absolute;
		font-family: inherit;
		display: inline-block;
		white-space: nowrap;
		color: inherit;
		opacity: 0;
		visibility: hidden;
		transition: opacity 300ms, visibility 300ms;
		right: 0;
		transform: translateX(100%);
		margin-right: -8px;
	}

	.tooltip.active {
		opacity: 1;
		visibility: initial;
	}
	.tooltip-slot:hover + .tooltip {
		opacity: 1;
		visibility: initial;
	}
</style>

<div class="relative inline-block">
	<span class="tooltip-slot">
		<slot />
	</span>
	<div
		class="tooltip"
		class:active
	>
	{#if tip}
		<div class="inline-block px-2 py-1 rounded border border-gray-700 bg-gray-100">{tip}</div>
	{/if}
</div>
</div>