<script setup lang="ts">
  defineProps<{
    modelValue: boolean
    label?: string
  }>()

  defineEmits<{
    (e: 'update:modelValue', value: boolean): void
  }>()
</script>

<template>
  <div class="toggle-switch">
    <span v-if="label" class="toggle-switch-label">{{ label }}</span>
    <label class="switch">
      <input
        type="checkbox"
        :checked="modelValue"
        @change="$emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
      />
      <span class="slider"></span>
    </label>
  </div>
</template>

<style scoped>
  .toggle-switch {
    display: flex;
    align-items: center;
    gap: 8px;
    /* Never let a flex parent squeeze the track - the knob would slide off the end. */
    flex: none;
  }

  .toggle-switch-label {
    font-size: 0.85em;
    color: #7e8898;
  }

  .switch {
    position: relative;
    display: inline-block;
    width: 34px;
    height: 18px;
  }

  .switch input {
    opacity: 0;
    width: 0;
    height: 0;
  }

  .slider {
    position: absolute;
    cursor: pointer;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background-color: #1f252e;
    transition: 0.3s;
    border-radius: 34px;
    border: 1px solid #2a303b;
  }

  .slider:before {
    position: absolute;
    content: '';
    height: 12px;
    width: 12px;
    left: 2px;
    bottom: 2px;
    background-color: #a7b4c7;
    transition: 0.3s;
    border-radius: 50%;
  }

  input:checked + .slider {
    background-color: #374151;
    border-color: #4b5563;
  }

  input:checked + .slider:before {
    transform: translateX(16px);
    background-color: #3ddc97;
  }
</style>
