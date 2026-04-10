/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.coordination.consul;

import com.orbitz.consul.Consul;
import com.orbitz.consul.ConsulException;
import com.orbitz.consul.KeyValueClient;
import com.orbitz.consul.SessionClient;
import com.orbitz.consul.model.kv.Value;
import com.orbitz.consul.model.session.ImmutableSession;
import com.orbitz.consul.model.session.SessionCreatedResponse;
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import lombok.extern.log4j.Log4j2;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Consul session-based {@link LeaderElection} implementation.
 *
 * <p>Leadership is determined by acquiring a Consul key with a session. Only one session
 * can hold a given key at a time. The session is created with TTL and "release" behavior,
 * meaning the key is released (not deleted) when the session expires, allowing other
 * participants to acquire it.</p>
 *
 * <p>A background renewal thread keeps the session alive. A separate contender thread
 * periodically attempts to acquire the election key if not currently the leader, and
 * detects leadership loss by checking session ownership.</p>
 *
 * <h2>Thread safety</h2>
 * <p>All state is managed via {@link AtomicBoolean} and {@link AtomicReference}.
 * Listener dispatch uses {@link CopyOnWriteArrayList}.</p>
 */
@Log4j2
final class ConsulLeaderElection implements LeaderElection {

    private static final String SESSION_TTL = "30s";
    private static final long RENEWAL_INTERVAL_MS = 10_000; // TTL/3
    private static final long CONTENDER_INTERVAL_MS = 5_000;

    private final Consul consul;
    private final String electionPath;
    private final String participantId;
    private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
    private final AtomicBoolean started;
    private final AtomicBoolean closed;
    private final AtomicBoolean leader;
    private final AtomicReference<String> sessionId;
    private volatile Thread renewalThread;
    private volatile Thread contenderThread;

    ConsulLeaderElection(Consul consul, String electionPath, String participantId) {
        this.consul = Objects.requireNonNull(consul, "consul");
        this.electionPath = Objects.requireNonNull(electionPath, "electionPath");
        this.participantId = Objects.requireNonNull(participantId, "participantId");
        this.listeners = new CopyOnWriteArrayList<>();
        this.started = new AtomicBoolean(false);
        this.closed = new AtomicBoolean(false);
        this.leader = new AtomicBoolean(false);
        this.sessionId = new AtomicReference<>(null);
    }

    @Override
    public void start() throws CoordinationException {
        if (closed.get()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Leader election is already closed: " + electionPath);
        }
        if (!started.compareAndSet(false, true)) {
            return;
        }
        try {
            String sid = createSession();
            sessionId.set(sid);
            log.debug("Created session {} for leader election on {}", sid, electionPath);

            // Start the session renewal daemon
            renewalThread = Thread.ofVirtual()
                    .name("consul-election-renewal-" + electionPath)
                    .start(this::renewalLoop);

            // Start the contender/leader-check daemon
            contenderThread = Thread.ofVirtual()
                    .name("consul-election-contender-" + electionPath)
                    .start(this::contenderLoop);

        } catch (ConsulException e) {
            started.set(false);
            throw ConsulCoordinationProvider.mapException(e, electionPath);
        }
    }

    @Override
    public boolean isLeader() {
        if (closed.get() || !started.get()) {
            return false;
        }
        return leader.get();
    }

    @Override
    public String currentLeaderId() throws CoordinationException {
        if (closed.get()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Leader election is closed: " + electionPath);
        }
        if (!started.get()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Leader election has not been started: " + electionPath);
        }
        try {
            KeyValueClient kvClient = consul.keyValueClient();
            Optional<Value> valueOpt = kvClient.getValue(electionPath);
            if (valueOpt.isEmpty()) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "No leader currently elected on " + electionPath);
            }
            Value value = valueOpt.get();
            // The value stored at the election key is the leader's participant ID
            Optional<String> valStr = value.getValueAsString();
            if (valStr.isEmpty() || value.getSession().isEmpty()) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "No leader currently elected on " + electionPath);
            }
            return valStr.get();
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw ConsulCoordinationProvider.mapException(e, electionPath);
        }
    }

    @Override
    public void addListener(LeaderChangeListener listener) {
        Objects.requireNonNull(listener, "listener");
        listeners.add(listener);
    }

    @Override
    public void close() throws IOException {
        if (!closed.compareAndSet(false, true)) {
            return;
        }

        // Interrupt background threads
        Thread rt = renewalThread;
        Thread ct = contenderThread;
        if (rt != null) {
            rt.interrupt();
        }
        if (ct != null) {
            ct.interrupt();
        }

        // Release the lock and destroy the session
        String sid = sessionId.getAndSet(null);
        if (sid != null) {
            try {
                KeyValueClient kvClient = consul.keyValueClient();
                kvClient.releaseLock(electionPath, sid);
            } catch (Exception e) {
                log.warn("Failed to release election key {} for session {}", electionPath, sid, e);
            }
            try {
                consul.sessionClient().destroySession(sid);
            } catch (Exception e) {
                log.warn("Failed to destroy session {} for election {}", sid, electionPath, e);
            }
        }

        if (leader.getAndSet(false)) {
            notifyListeners(false);
        }

        log.debug("Closed leader election on {} for participant {}", electionPath, participantId);
    }

    // ---- Internal ----

    private String createSession() {
        ImmutableSession session = ImmutableSession.builder()
                .name("election-" + electionPath + "-" + participantId)
                .ttl(SESSION_TTL)
                .behavior("release")
                .build();

        SessionCreatedResponse response = consul.sessionClient().createSession(session);
        return response.getId();
    }

    /**
     * Background loop that renews the session at TTL/3 interval to prevent expiry.
     */
    private void renewalLoop() {
        while (!closed.get() && !Thread.currentThread().isInterrupted()) {
            try {
                Thread.sleep(RENEWAL_INTERVAL_MS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
            String sid = sessionId.get();
            if (sid == null) {
                continue;
            }
            try {
                consul.sessionClient().renewSession(sid);
            } catch (ConsulException e) {
                log.warn("Failed to renew session {} for election {}, attempting to recreate",
                        sid, electionPath, e);
                handleSessionLoss();
            }
        }
    }

    /**
     * Background loop that periodically tries to acquire the election key
     * and detects leadership loss.
     */
    private void contenderLoop() {
        // Immediately try to acquire on start
        tryAcquireAndNotify();

        while (!closed.get() && !Thread.currentThread().isInterrupted()) {
            try {
                Thread.sleep(CONTENDER_INTERVAL_MS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
            tryAcquireAndNotify();
        }
    }

    private void tryAcquireAndNotify() {
        String sid = sessionId.get();
        if (sid == null || closed.get()) {
            return;
        }
        try {
            KeyValueClient kvClient = consul.keyValueClient();
            boolean wasLeader = leader.get();

            // Try to acquire the lock (writes participantId as value)
            boolean acquired = kvClient.acquireLock(electionPath, participantId, sid);

            if (acquired) {
                if (!wasLeader && leader.compareAndSet(false, true)) {
                    log.info("Participant {} became leader on path {}", participantId, electionPath);
                    notifyListeners(true);
                }
            } else {
                // Check if we still hold the lock (our session may still own it from before)
                Optional<Value> valueOpt = kvClient.getValue(electionPath);
                boolean stillLeader = valueOpt.isPresent()
                        && valueOpt.get().getSession().isPresent()
                        && valueOpt.get().getSession().get().equals(sid);

                if (stillLeader) {
                    if (!wasLeader && leader.compareAndSet(false, true)) {
                        log.info("Participant {} confirmed as leader on path {}", participantId, electionPath);
                        notifyListeners(true);
                    }
                } else {
                    if (wasLeader && leader.compareAndSet(true, false)) {
                        log.info("Participant {} lost leadership on path {}", participantId, electionPath);
                        notifyListeners(false);
                    }
                }
            }
        } catch (ConsulException e) {
            log.warn("Error during leader acquisition check on {}", electionPath, e);
            // On connection issues, assume we lost leadership
            if (leader.compareAndSet(true, false)) {
                log.info("Participant {} assumed leadership lost on {} due to error", participantId, electionPath);
                notifyListeners(false);
            }
        }
    }

    /**
     * Handles session loss by recreating the session and resuming contention.
     */
    private void handleSessionLoss() {
        if (leader.compareAndSet(true, false)) {
            notifyListeners(false);
        }
        if (closed.get()) {
            return;
        }
        try {
            String newSid = createSession();
            sessionId.set(newSid);
            log.info("Recreated session {} for election {} after session loss", newSid, electionPath);
        } catch (ConsulException e) {
            log.error("Failed to recreate session for election {}, will retry on next renewal cycle",
                    electionPath, e);
            sessionId.set(null);
        }
    }

    private void notifyListeners(boolean isLeader) {
        for (LeaderChangeListener listener : listeners) {
            try {
                listener.onLeaderChange(isLeader);
            } catch (Exception e) {
                log.error("Error notifying leader change listener on {}", electionPath, e);
            }
        }
    }
}
