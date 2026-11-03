<script setup lang="ts">
/**
 * Install commands per system, in tabs. Every tab is in the prerendered page for
 * search engines; the browser opens the visitor's own system, or the one the
 * address names (`#macos`).
 */
import { onMounted, ref } from 'vue'
import CopyCommand from '@/components/CopyCommand.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { detectPlatform } from '@/lib/platform'
import type { Dictionary } from '@/i18n/en'
import { INSTALL, type Platform } from '@/data/site'

const props = defineProps<{
  t: Dictionary['download']
  os: Dictionary['os']
  /** The option names' heading level, so it follows the page's outline. */
  level?: 'h2' | 'h3'
}>()

const platform = ref<Platform>('linux')
const ids: Platform[] = ['linux', 'macos', 'windows']

onMounted(() => {
  const hash = location.hash.slice(1)
  platform.value = (ids as string[]).includes(hash) ? (hash as Platform) : detectPlatform()
})
</script>

<template>
  <Tabs v-model="platform" class="min-w-0 gap-6">
    <TabsList class="h-11 w-full rounded-full bg-hearth p-1 sm:w-auto sm:self-start">
      <TabsTrigger v-for="id in ids" :key="id" :value="id" class="rounded-full px-5 text-[0.95rem]">{{ props.os[id] }}</TabsTrigger>
    </TabsList>
    <TabsContent
      v-for="id in ids"
      :key="id"
      :value="id"
      force-mount
      class="flex min-w-0 flex-col gap-4 data-[state=inactive]:hidden"
    >
      <p class="font-mono text-sm text-smoke">{{ props.t.platforms[id] }}</p>
      <div class="flex flex-col divide-y divide-grain overflow-hidden rounded-xl border border-grain bg-soot">
        <div v-for="option in INSTALL[id]" :key="option.id" class="grid min-w-0 gap-3 p-5 sm:grid-cols-[10rem_minmax(0,1fr)]">
          <component :is="props.level ?? 'h3'" class="font-display text-base font-bold text-paper">{{ props.t.options[option.id].label }}</component>
          <div class="flex min-w-0 flex-col gap-2">
            <CopyCommand v-for="command in option.commands" :key="command" :command="command" :copy-label="props.t.copy" :copied-label="props.t.copied" />
            <p v-if="props.t.options[option.id].note" class="text-[0.95rem] text-smoke">{{ props.t.options[option.id].note }}</p>
          </div>
        </div>
      </div>
    </TabsContent>
  </Tabs>
</template>
