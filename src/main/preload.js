'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('tirgumim', {
  info: () => ipcRenderer.invoke('app:info'),
  list: (reload) => ipcRenderer.invoke('games:list', { reload }),
  install: (id, opts) => ipcRenderer.invoke('game:install', id, opts),
  update: (id, opts) => ipcRenderer.invoke('game:update', id, opts),
  uninstall: (id) => ipcRenderer.invoke('game:uninstall', id),
  pickFolder: (id) => ipcRenderer.invoke('game:pickFolder', id),
  openFolder: (id) => ipcRenderer.invoke('game:openFolder', id),
  getSettings: () => ipcRenderer.invoke('settings:get'),
  setSettings: (s) => ipcRenderer.invoke('settings:set', s),
  onProgress: (cb) => ipcRenderer.on('progress', (_e, p) => cb(p)),
});
