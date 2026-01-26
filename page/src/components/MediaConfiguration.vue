<script setup lang="ts">
import { computed } from 'vue'
import type { MediaSettings, Settings } from '../types/settings'

const props = defineProps<{
  modelValue: MediaSettings
  defaultSettings: Settings
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: MediaSettings): void
}>()

const localMedia = computed({
  get: () => props.modelValue,
  set: (val) => emit('update:modelValue', val)
})

const onMediaChange = (field: keyof MediaSettings, value: any) => {
  localMedia.value = { ...localMedia.value, [field]: value }
}

const localSelectedFormat = computed({
  get: () => {
    if (localMedia.value.imageFormat === 'avif' && localMedia.value.imageAnimated) {
      return 'avif_animated'
    }
    if (localMedia.value.imageFormat === 'webp' && localMedia.value.imageAnimated) {
      return 'webp_animated'
    }
    return localMedia.value.imageFormat
  },
  set: (val: string) => {
    const newMedia = { ...localMedia.value }
    if (val === 'avif_animated') {
      newMedia.imageFormat = 'avif'
      newMedia.imageAnimated = true
      newMedia.imageQuality = 35
    } else if (val === 'webp_animated') {
      newMedia.imageFormat = 'webp'
      newMedia.imageAnimated = true
      newMedia.imageQuality = 75
    } else {
      newMedia.imageFormat = val as any
      newMedia.imageAnimated = false
      if (val === 'jpeg') newMedia.imageQuality = 5
      if (val === 'webp') newMedia.imageQuality = 80
      if (val === 'avif') newMedia.imageQuality = 25
    }
    localMedia.value = newMedia
  }
})

const qualityInfo = computed(() => {
  const isAnimated = localMedia.value.imageAnimated
  switch (localMedia.value.imageFormat) {
    case 'jpeg':
      return { label: 'q:v', min: 1, max: 31, hint: '↓ better', range: '1-31', default: 5 }
    case 'avif':
      return { 
        label: 'CRF', 
        min: 0, 
        max: 63, 
        hint: '↓ better', 
        range: '0-63', 
        default: isAnimated ? 35 : 25 
      }
    default:
      return { 
        label: 'Quality', 
        min: 0, 
        max: 100, 
        hint: '↑ better', 
        range: '0-100', 
        default: isAnimated ? 75 : 80 
      }
  }
})

function updateImageQuality(event: Event) {
  const val = parseInt((event.target as HTMLInputElement).value)
  if (!isNaN(val)) {
    const { min, max } = qualityInfo.value
    onMediaChange('imageQuality', Math.max(min, Math.min(max, val)))
  }
}

function updateAudioQuality(event: Event) {
  const input = event.target as HTMLInputElement
  const val = parseInt(input.value)
  if (!isNaN(val)) {
    const clamped = Math.max(8, Math.min(512, val))
    onMediaChange('audioQuality', clamped)
    if (val !== clamped) {
      input.value = clamped.toString()
    }
  }
}
</script>

<template>
  <div class="form-grid">
    <label class="form-group">
      <span>Start offset (seconds)</span>
      <div class="input-group">
        <input
          type="number"
          step="0.05"
          :value="localMedia.audioOffsetStart"
          @input="(e) => onMediaChange('audioOffsetStart', parseFloat((e.target as HTMLInputElement).value) || 0)"
        />
        <button 
          class="btn-reset" 
          :class="{ visible: localMedia.audioOffsetStart !== defaultSettings.media.audioOffsetStart }"
          :title="`Reset to default (${defaultSettings.media.audioOffsetStart}s)`"
          @click="onMediaChange('audioOffsetStart', defaultSettings.media.audioOffsetStart)"
        >
          ↺
        </button>
      </div>
    </label>
    <label class="form-group">
      <span>End offset (seconds)</span>
      <div class="input-group">
        <input
          type="number"
          step="0.05"
          :value="localMedia.audioOffsetEnd"
          @input="(e) => onMediaChange('audioOffsetEnd', parseFloat((e.target as HTMLInputElement).value) || 0)"
        />
        <button 
          class="btn-reset" 
          :class="{ visible: localMedia.audioOffsetEnd !== defaultSettings.media.audioOffsetEnd }"
          :title="`Reset to default (${defaultSettings.media.audioOffsetEnd}s)`"
          @click="onMediaChange('audioOffsetEnd', defaultSettings.media.audioOffsetEnd)"
        >
          ↺
        </button>
      </div>
    </label>
    <small class="field-hint full-width">Positive extends audio, negative trims it.</small>

    <div class="full-width separator"></div>

    <div class="full-width section-subheader-row">
      <span class="section-label">Image Settings</span>
      <div class="advanced-toggle">
        <span class="toggle-label">Advanced</span>
        <label class="switch">
          <input type="checkbox" v-model="localMedia.imageAdvanced">
          <span class="slider"></span>
        </label>
      </div>
    </div>

    <template v-if="!localMedia.imageAdvanced">
      <label class="form-group">
        <span>Image format</span>
        <select v-model="localSelectedFormat">
          <option value="jpeg">JPEG</option>
          <option value="webp">WebP (Still)</option>
          <option value="webp_animated">WebP (Animated)</option>
          <option value="avif">AVIF (Still)</option>
          <option value="avif_animated">AVIF (Animated)</option>
        </select>
      </label>
      <label class="form-group">
        <span>{{ qualityInfo.label }} <small class="subtle">({{ qualityInfo.range }}, {{ qualityInfo.hint }})</small></span>
        <div class="input-group">
          <input
            type="number"
            :min="qualityInfo.min"
            :max="qualityInfo.max"
            :value="localMedia.imageQuality"
            @input="updateImageQuality"
          />
          <button 
            class="btn-reset" 
            :class="{ visible: localMedia.imageQuality !== qualityInfo.default }"
            :title="`Reset to default (${qualityInfo.default})`"
            @click="localMedia.imageQuality = qualityInfo.default"
          >
            ↺
          </button>
        </div>
      </label>
      <label class="form-group">
        <span>Image resolution</span>
        <div class="input-group">
          <input
            type="text"
            v-model="localMedia.imageSize"
            placeholder="e.g. 640:-2"
          />
          <button 
            class="btn-reset" 
            :class="{ visible: localMedia.imageSize !== defaultSettings.media.imageSize }"
            :title="`Reset to default (${defaultSettings.media.imageSize})`"
            @click="localMedia.imageSize = defaultSettings.media.imageSize"
          >
            ↺
          </button>
        </div>
      </label>
    </template>
    <template v-else>
      <div class="advanced-row-header">
        <label class="form-group extension-box">
          <span>Extension</span>
          <input
            type="text"
            v-model="localMedia.imageAdvancedExtension"
            placeholder="jpg"
          />
        </label>
        <div class="advanced-toggle animated-switch-box">
          <span class="toggle-label">Animated</span>
          <label class="switch">
            <input type="checkbox" v-model="localMedia.imageAnimated">
            <span class="slider"></span>
          </label>
        </div>
      </div>
      <label class="form-group full-width">
        <span>Raw FFmpeg Arguments</span>
        <input
          type="text"
          v-model="localMedia.imageAdvancedArgs"
          placeholder="-c:v mjpeg -q:v 5"
        />
      </label>
    </template>

    <div class="full-width separator"></div>

    <div class="full-width section-subheader-row">
      <span class="section-label">Audio Settings</span>
      <div class="advanced-toggle">
        <span class="toggle-label">Advanced</span>
        <label class="switch">
          <input type="checkbox" v-model="localMedia.audioAdvanced">
          <span class="slider"></span>
        </label>
      </div>
    </div>

    <template v-if="!localMedia.audioAdvanced">
      <label class="form-group">
        <span>Audio format</span>
        <select v-model="localMedia.audioFormat">
          <option value="opus">Opus</option>
          <option value="mp3">MP3 (lame)</option>
        </select>
      </label>
      <label class="form-group">
        <span>Audio bitrate <small class="subtle">(kbps)</small></span>
        <div class="input-group">
          <input
            type="number"
            :min="8"
            :max="512"
            :value="localMedia.audioQuality"
            @input="updateAudioQuality"
          />
          <button 
            class="btn-reset" 
            :class="{ visible: localMedia.audioQuality !== defaultSettings.media.audioQuality }"
            :title="`Reset to default (${defaultSettings.media.audioQuality}kbps)`"
            @click="localMedia.audioQuality = defaultSettings.media.audioQuality"
          >
            ↺
          </button>
        </div>
      </label>
      <label class="form-group full-width">
        <span>Custom audio filters</span>
        <input
          type="text"
          v-model="localMedia.audioFilters"
          placeholder="e.g. dynaudnorm, volume=2.0"
        />
      </label>
    </template>
    <template v-else>
      <label class="form-group">
        <span>Extension</span>
        <input
          type="text"
          v-model="localMedia.audioAdvancedExtension"
          placeholder="mp3"
        />
      </label>
      <label class="form-group full-width">
        <span>Raw FFmpeg Arguments</span>
        <input
          type="text"
          v-model="localMedia.audioAdvancedArgs"
          placeholder="-c:a libmp3lame -b:a 128k (MP3 example)"
        />
      </label>
    </template>
  </div>
</template>

<style scoped>
/* Reuse existing styles */
.form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    align-items: end;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-group span {
    font-size: 0.85em;
    font-weight: 500;
    color: #a7b4c7;
  }

  .subtle {
    font-weight: 400;
    color: #7e8898;
  }

input[type='text'],
  input[type='number'],
  select {
    width: 100%;
    background: #11151b;
    border: 1px solid #2a303a;
    color: #e9edf2;
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 0.95em;
    outline: none;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
    -moz-appearance: textfield;
    appearance: none;
  }

  input::-webkit-outer-spin-button,
  input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    appearance: none;
    margin: 0;
  }

  input:focus,
  select:focus {
    border-color: #5a9aca;
    box-shadow: 0 0 0 2px rgba(90, 154, 202, 0.2);
  }

  .input-group {
    position: relative;
    display: flex;
    align-items: center;
  }

  .btn-reset {
    position: absolute;
    right: 8px;
    background: none;
    border: none;
    color: #7e8898;
    cursor: pointer;
    font-size: 1.1em;
    padding: 4px;
    line-height: 1;
    opacity: 0;
    transition:
      opacity 0.2s,
      color 0.2s,
      transform 0.2s;
    border-radius: 4px;
  }

  .btn-reset:hover {
    color: #ffd700;
    background: rgba(255, 255, 255, 0.05);
  }

  .btn-reset:active {
    transform: scale(0.9);
  }

  .btn-reset.visible {
    opacity: 1;
    pointer-events: auto;
  }

.full-width {
    grid-column: 1 / -1;
  }

  .advanced-row-header {
    display: flex;
    align-items: center;
    gap: 20px;
    margin-bottom: 10px;
  }

  .extension-box {
    width: 100px;
  }

  .animated-switch-box {
    margin-top: 15px; /* Alignment with input labels */
  }

  .separator {
    height: 1px;
    background: #1f252e;
    margin: 4px 0;
  }

  .section-subheader-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
    padding: 4px 0;
  }

  .section-label {
    font-weight: 600;
    color: #a7b4c7;
    font-size: 0.9em;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .advanced-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .toggle-label {
    font-size: 0.85em;
    color: #7e8898;
  }

  /* Switch styling */
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
    transition: .3s;
    border-radius: 34px;
    border: 1px solid #2a303b;
  }

  .slider:before {
    position: absolute;
    content: "";
    height: 12px;
    width: 12px;
    left: 2px;
    bottom: 2px;
    background-color: #a7b4c7;
    transition: .3s;
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
  
  .field-hint {
    color: #7e8898;
    font-size: 0.85em;
    margin-top: -4px;
    display: block;
  }
</style>
