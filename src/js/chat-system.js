/**
 * Chat System Module - Chat and voice call functionality
 * Uses window.Utils for utility functions
 */

(function() {
  'use strict';

  // Chat State
  let chatState = {
    currentServerId: 'home',
    currentChannelId: 'general',
    servers: [],
    channels: [],
    messages: {},
    userName: '',
    userColor: ''
  };

  // Voice Call State
  let voiceCallState = {
    localStream: null,
    remoteStream: null,
    peerConnection: null,
    isMuted: false,
    isDeafened: false,
    callStartTime: null,
    callTimerInterval: null
  };

  const RTC_CONFIG = {
    iceServers: [
      { urls: 'stun:stun.l.google.com:19302' },
      { urls: 'stun:stun1.l.google.com:19302' }
    ]
  };

  /**
   * Initialize chat system
   */
  const initChat = async () => {
    try {
      chatState.userName = window.Utils.getUserName() || 'User';
      chatState.userColor = localStorage.getItem('audio-viz-theme-color') || '#58c2ff';
      
      updateChatUserInfo();
      showChatWelcome();
      console.log('Chat initialized');
    } catch (error) {
      console.error('Failed to initialize chat:', error);
      window.Utils.logToUI('error', 'Chat initialization failed');
    }
  };

  /**
   * Update chat user info
   */
  const updateChatUserInfo = () => {
    const avatarEl = document.getElementById('chat-user-avatar');
    const usernameEl = document.getElementById('chat-username');

    if (avatarEl) {
      avatarEl.textContent = chatState.userName.charAt(0).toUpperCase();
      avatarEl.style.background = chatState.userColor;
    }
    if (usernameEl) {
      usernameEl.textContent = chatState.userName;
    }
  };

  /**
   * Show chat welcome state
   */
  const showChatWelcome = () => {
    const channelNameEl = document.getElementById('current-channel-name');
    const welcomeNameEls = document.querySelectorAll('#welcome-channel-name, #welcome-channel-name-2');
    const inputEl = document.getElementById('chat-input');

    if (channelNameEl) channelNameEl.textContent = chatState.currentChannelId;
    welcomeNameEls.forEach(el => el.textContent = chatState.currentChannelId);
    if (inputEl) inputEl.placeholder = `Message #${chatState.currentChannelId}`;
  };

  /**
   * Send chat message
   */
  const sendMessage = async () => {
    const inputEl = document.getElementById('chat-input');
    if (!inputEl || !inputEl.value.trim()) return;

    const content = inputEl.value.trim();
    inputEl.value = '';

    addMessageToUI({
      id: Date.now(),
      channel_id: chatState.currentChannelId,
      user_name: chatState.userName,
      user_color: chatState.userColor,
      content,
      timestamp: new Date().toISOString(),
      edited: false,
      has_attachment: false,
      is_eq_preset: false
    });
  };

  /**
   * Add message to UI
   */
  const addMessageToUI = (message) => {
    const messagesContainer = document.getElementById('chat-messages');
    const welcomeEl = document.getElementById('message-welcome');
    if (!messagesContainer) return;

    if (welcomeEl) welcomeEl.style.display = 'none';

    const messageEl = createMessageElement(message);
    messagesContainer.appendChild(messageEl);
    messagesContainer.scrollTop = messagesContainer.scrollHeight;
  };

  /**
   * Create message element
   */
  const createMessageElement = (message) => {
    const el = document.createElement('div');
    el.className = `message ${message.is_eq_preset ? 'eq-preset' : ''}`;
    el.dataset.messageId = message.id;

    const avatarInitial = message.user_name.charAt(0).toUpperCase();
    const timestamp = new Date(message.timestamp).toLocaleString();

    el.innerHTML = `
      <div class="message-avatar" style="background: ${message.user_color || 'var(--text-primary)'}">
        ${avatarInitial}
      </div>
      <div class="message-content">
        <div class="message-header">
          <span class="message-author" style="color: ${message.user_color || 'var(--text-primary)'}">
            ${window.Utils.escapeHtml(message.user_name)}
          </span>
          <span class="message-timestamp">${timestamp}</span>
          ${message.edited ? '<span class="message-edited">(edited)</span>' : ''}
          ${message.is_eq_preset ? '<span class="eq-preset-badge">EQ Preset</span>' : ''}
        </div>
        <div class="message-text">${window.Utils.escapeHtml(message.content)}</div>
      </div>
    `;

    if (message.is_eq_preset) {
      el.addEventListener('click', () => {
        window.Utils.showNotification('EQ preset loading not yet implemented', 'info');
      });
    }

    return el;
  };

  /**
   * Select server
   */
  const selectServer = async (serverId) => {
    chatState.currentServerId = serverId;

    document.querySelectorAll('.server-item').forEach(el => {
      el.classList.toggle('active', el.dataset.serverId === serverId);
    });

    const serverNameEl = document.getElementById('current-server-name');
    const server = document.querySelector(`.server-item[data-server-id="${serverId}"]`);
    if (serverNameEl && server) {
      serverNameEl.textContent = server.title || 'Server';
    }
  };

  /**
   * Select channel
   */
  const selectChannel = async (channelId, channelName) => {
    chatState.currentChannelId = channelId;

    document.querySelectorAll('.channel-item').forEach(el => {
      el.classList.toggle('active', el.dataset.channelId === channelId);
    });

    const channelNameEl = document.getElementById('current-channel-name');
    const inputEl = document.getElementById('chat-input');
    const welcomeNameEls = document.querySelectorAll('#welcome-channel-name, #welcome-channel-name-2');

    if (channelNameEl) channelNameEl.textContent = channelName;
    if (inputEl) inputEl.placeholder = `Message #${channelName}`;
    welcomeNameEls.forEach(el => el.textContent = channelName);

    const messagesContainer = document.getElementById('chat-messages');
    const welcomeEl = document.getElementById('message-welcome');
    if (messagesContainer) {
      messagesContainer.querySelectorAll('.message').forEach(el => el.remove());
    }
    if (welcomeEl) welcomeEl.style.display = 'block';
  };

  /**
   * Show create server modal
   */
  const showCreateServerModal = () => {
    const modal = document.getElementById('create-server-modal');
    if (modal) modal.style.display = 'flex';
  };

  /**
   * Hide create server modal
   */
  const hideCreateServerModal = () => {
    const modal = document.getElementById('create-server-modal');
    if (modal) modal.style.display = 'none';
  };

  /**
   * Create server
   */
  const createServer = async () => {
    const nameInput = document.getElementById('new-server-name');
    const name = nameInput?.value.trim();
    if (!name) return;

    addServerToUI(name, Date.now());
    nameInput.value = '';
    hideCreateServerModal();
    window.Utils.showNotification('Server created!', 'success');
  };

  /**
   * Add server to UI
   */
  const addServerToUI = (name, id) => {
    const serverList = document.getElementById('dynamic-server-list');
    if (!serverList) return;

    const serverEl = document.createElement('div');
    serverEl.className = 'server-item';
    serverEl.dataset.serverId = id;
    serverEl.title = name;
    serverEl.textContent = name.charAt(0).toUpperCase();
    serverEl.addEventListener('click', () => selectServer(id.toString()));
    serverList.appendChild(serverEl);
  };

  /**
   * Show share EQ preset modal
   */
  const showShareEqPresetModal = () => {
    const modal = document.getElementById('share-eq-preset-modal');
    if (modal) modal.style.display = 'flex';
  };

  /**
   * Hide share EQ preset modal
   */
  const hideShareEqPresetModal = () => {
    const modal = document.getElementById('share-eq-preset-modal');
    if (modal) modal.style.display = 'none';
  };

  /**
   * Share EQ preset
   */
  const shareEqPreset = () => {
    const nameInput = document.getElementById('eq-preset-name');
    const name = nameInput?.value.trim();
    if (!name) return;

    addMessageToUI({
      id: Date.now(),
      channel_id: chatState.currentChannelId,
      user_name: chatState.userName,
      user_color: chatState.userColor,
      content: `Shared EQ Preset: ${name}`,
      timestamp: new Date().toISOString(),
      edited: false,
      has_attachment: false,
      is_eq_preset: true
    });

    nameInput.value = '';
    hideShareEqPresetModal();
    window.Utils.showNotification('EQ Preset shared to chat!', 'success');
  };

  /**
   * Toggle members sidebar
   */
  const toggleMembersSidebar = () => {
    const sidebar = document.getElementById('chat-members-sidebar');
    if (sidebar) {
      const isVisible = sidebar.style.display !== 'none';
      sidebar.style.display = isVisible ? 'none' : 'block';

      if (!isVisible && sidebar.querySelectorAll('.member-item').length === 0) {
        addDemoMembers();
      }
    }
  };

  /**
   * Add demo members
   */
  const addDemoMembers = () => {
    const membersList = document.getElementById('members-list');
    if (!membersList) return;

    const demoMembers = [
      { name: chatState.userName, status: 'online' },
      { name: 'Friend1', status: 'online' },
      { name: 'Friend2', status: 'idle' },
      { name: 'Friend3', status: 'offline' }
    ];

    demoMembers.forEach(member => {
      const memberEl = document.createElement('div');
      memberEl.className = 'member-item';
      memberEl.innerHTML = `
        <div class="member-avatar">${member.name.charAt(0).toUpperCase()}</div>
        <span class="member-name">${member.name}</span>
        <div class="member-status ${member.status === 'offline' ? 'offline' : ''}"></div>
      `;
      membersList.appendChild(memberEl);
    });
  };

  /**
   * Toggle mute
   */
  const toggleMute = () => {
    if (voiceCallState.localStream) {
      const audioTrack = voiceCallState.localStream.getAudioTracks()[0];
      if (audioTrack) {
        voiceCallState.isMuted = !voiceCallState.isMuted;
        audioTrack.enabled = !voiceCallState.isMuted;

        const muteBtn = document.getElementById('voice-mute-btn');
        if (muteBtn) {
          muteBtn.classList.toggle('muted', voiceCallState.isMuted);
        }
      }
    }
  };

  /**
   * Toggle deafen
   */
  const toggleDeafen = () => {
    voiceCallState.isDeafened = !voiceCallState.isDeafened;

    const deafenBtn = document.getElementById('voice-deafen-btn');
    if (deafenBtn) {
      deafenBtn.classList.toggle('muted', voiceCallState.isDeafened);
    }

    if (voiceCallState.isDeafened && !voiceCallState.isMuted) {
      toggleMute();
    }

    window.Utils.showNotification(
      voiceCallState.isDeafened ? "Deafened - You won't hear others" : 'Undeafened', 
      'info'
    );
  };

  /**
   * Start call timer
   */
  const startCallTimer = () => {
    voiceCallState.callStartTime = Date.now();
    voiceCallState.callTimerInterval = setInterval(() => {
      const elapsed = Math.floor((Date.now() - voiceCallState.callStartTime) / 1000);
      const minutes = Math.floor(elapsed / 60).toString().padStart(2, '0');
      const seconds = (elapsed % 60).toString().padStart(2, '0');

      const timerEl = document.getElementById('call-timer');
      if (timerEl) {
        timerEl.textContent = `${minutes}:${seconds}`;
      }
    }, 1000);
  };

  /**
   * End call
   */
  const endCall = () => {
    if (voiceCallState.peerConnection) {
      voiceCallState.peerConnection.close();
      voiceCallState.peerConnection = null;
    }

    if (voiceCallState.localStream) {
      voiceCallState.localStream.getTracks().forEach(track => track.stop());
      voiceCallState.localStream = null;
    }

    if (voiceCallState.callTimerInterval) {
      clearInterval(voiceCallState.callTimerInterval);
      voiceCallState.callTimerInterval = null;
    }

    hideVoiceCallModal();
  };

  /**
   * Show voice call modal
   */
  const showVoiceCallModal = (participantName) => {
    const modal = document.getElementById('voice-call-modal');
    const nameEl = document.getElementById('call-participant-name');
    const avatarEl = document.getElementById('call-participant-avatar');

    if (modal) modal.style.display = 'flex';
    if (nameEl) nameEl.textContent = participantName;
    if (avatarEl) avatarEl.textContent = participantName.charAt(0).toUpperCase();

    startCallTimer();
  };

  /**
   * Hide voice call modal
   */
  const hideVoiceCallModal = () => {
    const modal = document.getElementById('voice-call-modal');
    if (modal) modal.style.display = 'none';
  };

  /**
   * Show incoming call modal
   */
  const showIncomingCallModal = (callerId, callerName, sdp) => {
    const modal = document.getElementById('incoming-call-modal');
    const nameEl = document.getElementById('incoming-caller-name');
    const avatarEl = document.getElementById('incoming-call-avatar');

    if (modal) modal.style.display = 'flex';
    if (nameEl) nameEl.textContent = `${callerName} is calling you...`;
    if (avatarEl) avatarEl.textContent = callerName.charAt(0).toUpperCase();
  };

  /**
   * Hide incoming call modal
   */
  const hideIncomingCallModal = () => {
    const modal = document.getElementById('incoming-call-modal');
    if (modal) modal.style.display = 'none';
  };

  /**
   * Accept call
   */
  const acceptCall = async (callerId, sdp) => {
    try {
      voiceCallState.localStream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true
        }
      });

      hideIncomingCallModal();
      showVoiceCallModal(callerId);
      startCallTimer();
    } catch (error) {
      console.error('Failed to accept call:', error);
      window.Utils.logToUI('error', 'Failed to accept call');
      window.Utils.showNotification('Failed to accept call: ' + window.Utils.sanitizeErrorMessage(error), 'error');
    }
  };

  // Wrap all DOM queries and Tauri calls in DOMContentLoaded
  window.ChatSystem = {
    initChat,
    updateChatUserInfo,
    showChatWelcome,
    sendMessage,
    addMessageToUI,
    createMessageElement,
    selectServer,
    selectChannel,
    showCreateServerModal,
    hideCreateServerModal,
    createServer,
    showShareEqPresetModal,
    hideShareEqPresetModal,
    shareEqPreset,
    toggleMembersSidebar,
    toggleMute,
    toggleDeafen,
    endCall,
    showVoiceCallModal,
    hideVoiceCallModal,
    showIncomingCallModal,
    hideIncomingCallModal,
    acceptCall
  };

  console.log('[ChatSystem] Module initialized');
  window.Utils.logToUI('info', 'ChatSystem module ready');
})();
