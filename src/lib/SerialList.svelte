<script lang="ts">
    export let values = [];
    export let value: string;
</script>

<select
    bind:value={value}
    class="w-24 bg-gray-200 border border-gray-500 text-gray-700 py-1 px-2 rounded leading-tight focus:outline-none focus:bg-white focus:border-gray-500"
>
    {#each values as option }
        <option value={option}>{option}</option>
    {/each}
</select>

<style>
    
</style>