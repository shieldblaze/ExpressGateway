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
package com.shieldblaze.expressgateway.coordination.zookeeper;

import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import lombok.extern.log4j.Log4j2;
import org.apache.curator.framework.recipes.leader.LeaderLatch;
import org.apache.curator.framework.recipes.leader.LeaderLatchListener;
import org.apache.zookeeper.KeeperException;

import java.io.IOException;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * ZooKeeper-backed {@link LeaderElection} wrapping Curator's {@link LeaderLatch}.
 *
 * <p>The latch is created in the constructor but NOT started until {@link #start()}
 * is called. This matches the CoordinationProvider contract where
 * {@code leaderElection()} returns a handle that must be explicitly started.</p>
 *
 * <p>Thread safety: {@link LeaderLatch} is internally thread-safe.
 * Listener dispatch uses {@link CopyOnWriteArrayList} for safe concurrent iteration.</p>
 */
@Log4j2
final class ZooKeeperLeaderElection implements LeaderElection {

    private final LeaderLatch latch;
    private final String electionPath;
    private final String participantId;
    private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
    private final AtomicBoolean started;
    private final AtomicBoolean closed;

    ZooKeeperLeaderElection(LeaderLatch latch, String electionPath, String participantId) {
        this.latch = latch;
        this.electionPath = electionPath;
        this.participantId = participantId;
        this.listeners = new CopyOnWriteArrayList<>();
        this.started = new AtomicBoolean(false);
        this.closed = new AtomicBoolean(false);

        // Register the Curator listener that bridges to our LeaderChangeListener API.
        // This must be done before start() so no events are missed.
        latch.addListener(new LeaderLatchListener() {
            @Override
            public void isLeader() {
                log.info("Participant {} became leader on path {}", participantId, electionPath);
                notifyListeners(true);
            }

            @Override
            public void notLeader() {
                log.info("Participant {} lost leadership on path {}", participantId, electionPath);
                notifyListeners(false);
            }
        });
    }

    @Override
    public void start() throws CoordinationException {
        if (closed.get()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Leader election is already closed: " + electionPath);
        }
        if (!started.compareAndSet(false, true)) {
            // Idempotent: already started
            return;
        }
        try {
            latch.start();
            log.debug("Started leader election on {} with participant {}", electionPath, participantId);
        } catch (Exception e) {
            started.set(false);
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to start leader election on " + electionPath, e);
        }
    }

    @Override
    public boolean isLeader() {
        if (closed.get() || !started.get()) {
            return false;
        }
        try {
            return latch.hasLeadership();
        } catch (IllegalStateException _) {
            // LeaderLatch throws if not started or already closed
            return false;
        }
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
            return latch.getLeader().getId();
        } catch (KeeperException.NoNodeException e) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "No leader currently elected on " + electionPath, e);
        } catch (Exception e) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to determine current leader on " + electionPath, e);
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
        if (started.get()) {
            latch.close(LeaderLatch.CloseMode.NOTIFY_LEADER);
            log.debug("Closed leader election on {} for participant {}", electionPath, participantId);
        }
    }

    /**
     * Notifies all registered listeners of a leadership state change.
     * Exceptions from individual listeners are logged and swallowed.
     */
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
