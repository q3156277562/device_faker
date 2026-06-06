<template>
  <div class="template-page">
    <Transition :name="viewTransitionName" @after-enter="handleViewAfterEnter">
      <OnlineTemplateLibraryView
        v-if="onlineTemplatesStore.libraryOpen"
        key="online-library"
        @close="closeOnlineLibrary"
      />

      <div v-else key="template-list" class="template-home-view">
        <TemplateHeader
          :locale="locale"
          @open-online="showOnlineLibrary"
          @open-create="showCreateDialog"
          @open-transfer="showTransferDialog"
          @search="handleSearch"
        />

        <TemplateList
          :entries="filteredTemplates"
          :is-searching="searchQuery.length > 0"
          @export="handleExport"
          @edit="handleEdit"
          @delete="deleteTemplateConfirm"
        />

        <TemplateDialog
          v-if="dialogVisible"
          v-model="dialogVisible"
          :is-editing="isEditing"
          :locale="locale"
          :template-name="editingTemplateName"
          :template-data="editingTemplate"
          @saved="handleTemplateSaved"
        />

        <TemplateTransferDialog v-if="transferDialogVisible" v-model="transferDialogVisible" />
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { computed, defineAsyncComponent, onActivated, ref } from 'vue'
import { storeToRefs } from 'pinia'
import TemplateHeader from '../components/templates/TemplateHeader.vue'
import TemplateList from '../components/templates/TemplateList.vue'
import OnlineTemplateLibraryView from '../components/OnlineTemplateLibraryView.vue'
import { useConfigStore } from '../stores/config'
import { useOnlineTemplatesStore } from '../stores/onlineTemplates'
import { useModalHistory } from '../composables/useModalHistory'
import { useI18n } from '../utils/i18n'
import { useLazyMessageBox } from '../utils/elementPlus'
import { copyTextToClipboard, stringifyTemplatesToToml } from '../utils/templateTransfer'
import { toast } from 'kernelsu-alt'
import type { Template } from '../types'

const TemplateDialog = defineAsyncComponent(
  () => import('../components/templates/TemplateDialog.vue')
)
const TemplateTransferDialog = defineAsyncComponent(
  () => import('../components/templates/TemplateTransferDialog.vue')
)

const configStore = useConfigStore()
const onlineTemplatesStore = useOnlineTemplatesStore()
const { libraryOpen } = storeToRefs(onlineTemplatesStore)
const { t, locale } = useI18n()
const getMessageBox = useLazyMessageBox()

const searchQuery = ref('')
const viewTransitionName = ref<'template-library-forward' | 'template-library-back'>(
  'template-library-forward'
)

const allTemplates = computed(() => configStore.templateEntries)

const filteredTemplates = computed(() => {
  if (!searchQuery.value.trim()) {
    return allTemplates.value
  }

  const query = searchQuery.value.toLowerCase().trim()

  return allTemplates.value.filter(({ name, template }) => {
    const searchFields = [
      name,
      template.brand || '',
      template.model || '',
      template.build_id || '',
      template.device || '',
      template.manufacturer || '',
      template.product || '',
      template.timezone || '',
    ]

    const matches = searchFields.some((field) => field.toLowerCase().includes(query))
    return matches
  })
})

function handleSearch(query: string) {
  searchQuery.value = query
}

const dialogVisible = ref(false)
const transferDialogVisible = ref(false)
const isEditing = ref(false)
const editingTemplateName = ref<string | null>(null)
const editingTemplate = ref<Template | null>(null)

function showOnlineLibrary() {
  viewTransitionName.value = 'template-library-forward'
  onlineTemplatesStore.openLibrary()
  void onlineTemplatesStore.ensureCatalogLoaded()
}

function closeOnlineLibrary() {
  viewTransitionName.value = 'template-library-back'
  onlineTemplatesStore.closeLibrary()
}

function handleViewAfterEnter() {
  window.dispatchEvent(new Event('resize'))
}

useModalHistory(libraryOpen, closeOnlineLibrary)
useModalHistory(dialogVisible, () => {
  dialogVisible.value = false
})
useModalHistory(transferDialogVisible, () => {
  transferDialogVisible.value = false
})

function showCreateDialog() {
  isEditing.value = false
  editingTemplateName.value = null
  editingTemplate.value = null
  dialogVisible.value = true
}

function showTransferDialog() {
  transferDialogVisible.value = true
}

function handleEdit(name: string, template: Template) {
  isEditing.value = true
  editingTemplateName.value = name
  editingTemplate.value = template
  dialogVisible.value = true
}

async function handleExport(name: string, template: Template) {
  try {
    const content = stringifyTemplatesToToml({ [name]: template })
    const copied = await copyTextToClipboard(content)
    toast(
      copied
        ? t('templates.messages.export_copy_success')
        : t('templates.messages.export_copy_failed')
    )
  } catch (error) {
    console.error('Export template failed:', error)
    toast(t('templates.messages.export_copy_failed'))
  }
}

async function deleteTemplateConfirm(name: string) {
  try {
    const messageBox = await getMessageBox()
    await messageBox.confirm(
      t('templates.dialog.delete_confirm', { name }),
      t('templates.dialog.delete_title'),
      {
        confirmButtonText: t('common.delete'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
        appendTo: 'body',
        customClass: 'delete-confirm-box',
        modalClass: 'delete-confirm-modal',
      }
    )

    configStore.deleteTemplate(name)
    await configStore.saveConfig()
    toast(t('templates.messages.deleted'))
  } catch (e) {
    if (e === 'cancel') return
    console.error('Delete template failed:', e)
    const errorMessage = e instanceof Error ? e.message : String(e)
    toast(`${t('common.failed')}: ${errorMessage}`)
  }
}

function handleTemplateSaved() {
  // 保存后无需额外动作，保留扩展点
}

onActivated(() => {
  // KeepAlive 激活时触发一次尺寸计算，确保列表布局正确
  window.dispatchEvent(new Event('resize'))
})
</script>

<style scoped>
.template-page {
  --template-view-slide-distance: 1.25rem;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1rem;
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  overflow: hidden;
}

.template-home-view {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  width: 100%;
}

.template-library-forward-enter-active,
.template-library-forward-leave-active,
.template-library-back-enter-active,
.template-library-back-leave-active {
  transition:
    opacity 220ms ease,
    transform 260ms cubic-bezier(0.22, 1, 0.36, 1);
  will-change: opacity, transform;
}

.template-library-forward-leave-active,
.template-library-back-leave-active {
  position: absolute;
  inset: 0;
  z-index: 1;
  width: 100%;
}

.template-library-forward-enter-active,
.template-library-back-enter-active {
  position: relative;
  z-index: 2;
}

.template-library-forward-enter-from {
  opacity: 0;
  transform: translate3d(var(--template-view-slide-distance), 0, 0);
}

.template-library-forward-leave-to {
  opacity: 0;
  transform: translate3d(calc(var(--template-view-slide-distance) * -1), 0, 0);
}

.template-library-back-enter-from {
  opacity: 0;
  transform: translate3d(calc(var(--template-view-slide-distance) * -1), 0, 0);
}

.template-library-back-leave-to {
  opacity: 0;
  transform: translate3d(var(--template-view-slide-distance), 0, 0);
}

@media (prefers-reduced-motion: reduce) {
  .template-library-forward-enter-active,
  .template-library-forward-leave-active,
  .template-library-back-enter-active,
  .template-library-back-leave-active {
    transition-duration: 1ms;
  }

  .template-library-forward-enter-from,
  .template-library-forward-leave-to,
  .template-library-back-enter-from,
  .template-library-back-leave-to {
    transform: none;
  }
}
</style>

<style>
.delete-confirm-modal {
  backdrop-filter: blur(12px) saturate(120%) !important;
  -webkit-backdrop-filter: blur(12px) saturate(120%) !important;
  background-color: rgba(0, 0, 0, 0.15) !important;
}

.dark .delete-confirm-modal {
  backdrop-filter: blur(12px) saturate(120%) !important;
  -webkit-backdrop-filter: blur(12px) saturate(120%) !important;
  background-color: rgba(0, 0, 0, 0.4) !important;
}

.delete-confirm-box {
  background: rgba(255, 255, 255, 0.95) !important;
  backdrop-filter: blur(40px) saturate(150%) brightness(1.1) !important;
  -webkit-backdrop-filter: blur(40px) saturate(150%) brightness(1.1) !important;
  border: 1px solid rgba(0, 0, 0, 0.1) !important;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.1) !important;
}

.dark .delete-confirm-box {
  background: rgba(20, 20, 20, 0.6) !important;
  backdrop-filter: blur(40px) saturate(150%) brightness(0.9) !important;
  -webkit-backdrop-filter: blur(40px) saturate(150%) brightness(0.9) !important;
  border: 1px solid rgba(255, 255, 255, 0.15) !important;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5) !important;
}
</style>
