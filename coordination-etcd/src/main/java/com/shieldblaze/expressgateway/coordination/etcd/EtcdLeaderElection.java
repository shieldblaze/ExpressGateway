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
package com.shieldblaze.expressgateway.coordination.etcd;

import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.Election;
import io.etcd.jetcd.Lease;
import io.etcd.jetcd.election.CampaignResponse;
import io.etcd.jetcd.election.LeaderKey;
import io.etcd.jetcd.election.LeaderResponse;
import io.etcd.jetcd.lease.LeaseKeepAliveResponse;
import io.etcd.jetcd.support.CloseableClient;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/**
 * etcd-backed {@link LeaderElection} using the etcd Election API (v3election).
 *
 * <p>The election is driven by a dedicated lease. When {@link #start()} is called,
 * a background virtual thread campaigns for leadership. Leadership state transitions
 * are tracked via an {@link AtomicBoolean} and dispatched to registered listeners.</p>
 *
 * <p>The etcd Election API uses a lease-based approach: the leader must keep its
 * lease alive. If the lease expires (process crash, network partition), the leader
 * key is deleted and a new leader is elected automatically.</p>
 *
 * <h2>Thread safety</h2>
 * <p>All mutable state is managed through atomics and CopyOnWriteArrayList.
 * The campaign runs on a virtual thread to avoid blocking platform threads.</p>
 */
@Log4j2
final class EtcdLeaderElection implements LeaderElection {

    private static final long LEASE_TTL_SECONDS = 15;

    private final Client client;
    private final String electionPath;
    private final String participantId;
    private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
    private final AtomicBoolean isLeaderFlag;
    private final AtomicBoolean started;
    private final AtomicBoolean closed;
    private final AtomicReference<LeaderKey> leaderKeyRef;

    private volatile long leaseId;
    private volatile CloseableClient keepAliveHandle;
    private volatile Thread campaignThread;

    EtcdLeaderElection(Client client, String electionPath, String participantId) {
        this.client = Objects.requireNonNull(client, "client");
        this.electionPath = Objects.requireNonNull(electionPath, "electionPath");
        this.participantId = Objects.requireNonNull(participantId, "participantId");
        this.listeners = new CopyOnWriteArrayList<>();
        this.isLeaderFlag = new AtomicBoolean(false);
        this.started = new AtomicBoolean(false);
        this.closed = new AtomicBoolean(false);
        this.leaderKeyRef = new AtomicReference<>();
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
            Lease leaseClient = client.getLeaseClient();

            // Grant a lease for the election
            leaseId = leaseClient.grant(LEASE_TTL_SECONDS).get(10, TimeUnit.SECONDS).getID();

            // Start keep-alive to prevent lease expiry
            keepAliveHandle = leaseClient.keepAlive(leaseId, new StreamObserver<>() {
                @Override
                public void onNext(LeaseKeepAliveResponse response) {
                    log.trace("Election lease {} keep-alive renewed, TTL={}", leaseId, response.getTTL());
                }

                @Override
                public void onError(Throwable t) {
                    log.warn("Election lease {} keep-alive error on {}", leaseId, electionPath, t);
                    // Lease keep-alive failure likely means we lost leadership
                    if (isLeaderFlag.getAndSet(false)) {
                        notifyListeners(false);
                    }
                }

                @Override
                public void onCompleted() {
                    log.debug("Election lease {} keep-alive completed on {}", leaseId, electionPath);
                }
            });

            log.debug("Created lease {} with TTL {}s for election on {}",
                    leaseId, LEASE_TTL_SECONDS, electionPath);

        } catch (Exception e) {
            started.set(false);
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to create lease for leader election on " + electionPath, e);
        }

        // Campaign on a virtual thread so we don't block platform threads.
        // campaign() blocks until this participant becomes the leader.
        campaignThread = Thread.ofVirtual()
                .name("etcd-election-" + electionPath)
                .start(this::campaignLoop);
    }

    /**
     * Campaign loop that runs on a virtual thread. The etcd campaign call blocks
     * until this participant becomes the leader. If the campaign is interrupted
     * (e.g. on close()), the loop exits.
     */
    private void campaignLoop() {
        Election electionClient = client.getElectionClient();
        ByteSequence electionName = bs(electionPath);
        ByteSequence proposal = bs(participantId);

        while (!closed.get() && !Thread.currentThread().isInterrupted()) {
            try {
                // campaign() blocks until this node wins the election
                CampaignResponse response = electionClient.campaign(electionName, leaseId, proposal)
                        .get();

                LeaderKey key = response.getLeader();
                leaderKeyRef.set(key);

                if (!isLeaderFlag.getAndSet(true)) {
                    log.info("Participant {} became leader on path {} (key={}, rev={})",
                            participantId, electionPath, str(key.getKey()), key.getRevision());
                    notifyListeners(true);
                }

                // Observe leadership changes. This blocks while we remain leader.
                observeLeadership(electionClient, electionName);

            } catch (ExecutionException e) {
                if (closed.get() || Thread.currentThread().isInterrupted()) {
                    break;
                }
                log.error("Election campaign failed for {} on {}, retrying in 1s",
                        participantId, electionPath, e.getCause());
                if (isLeaderFlag.getAndSet(false)) {
                    notifyListeners(false);
                }
                try {
                    Thread.sleep(1000);
                } catch (InterruptedException ie) {
                    Thread.currentThread().interrupt();
                    break;
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
        }

        // Exiting the loop means we are no longer a leader
        if (isLeaderFlag.getAndSet(false)) {
            notifyListeners(false);
        }
    }

    /**
     * Observes the election and blocks until our leadership is lost.
     * Periodically polls the leader endpoint to detect leadership changes.
     */
    private void observeLeadership(Election electionClient, ByteSequence electionName) {
        while (!closed.get() && !Thread.currentThread().isInterrupted()) {
            try {
                LeaderResponse leaderResponse = electionClient.leader(electionName)
                        .get(LEASE_TTL_SECONDS, TimeUnit.SECONDS);

                String currentLeader = str(leaderResponse.getKv().getValue());

                if (!participantId.equals(currentLeader)) {
                    // We lost leadership
                    if (isLeaderFlag.getAndSet(false)) {
                        log.info("Participant {} lost leadership on path {} (new leader: {})",
                                participantId, electionPath, currentLeader);
                        notifyListeners(false);
                    }
                    return; // Re-enter campaign loop to re-campaign
                }

                // Still the leader -- sleep briefly before re-checking
                Thread.sleep(2000);

            } catch (Exception e) {
                if (closed.get() || Thread.currentThread().isInterrupted()) {
                    return;
                }
                // Transient error checking leadership -- assume lost
                log.warn("Error observing leadership on {}, assuming lost", electionPath, e);
                if (isLeaderFlag.getAndSet(false)) {
                    notifyListeners(false);
                }
                return;
            }
        }
    }

    @Override
    public boolean isLeader() {
        if (closed.get() || !started.get()) {
            return false;
        }
        return isLeaderFlag.get();
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
            Election electionClient = client.getElectionClient();
            LeaderResponse response = electionClient.leader(bs(electionPath))
                    .get(10, TimeUnit.SECONDS);
            return str(response.getKv().getValue());
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "No leader currently elected on " + electionPath, cause);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while querying leader on " + electionPath, e);
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

        // Interrupt the campaign thread first
        Thread ct = campaignThread;
        if (ct != null) {
            ct.interrupt();
        }

        // Resign from the election if we hold a leader key
        LeaderKey key = leaderKeyRef.getAndSet(null);
        if (key != null) {
            try {
                client.getElectionClient().resign(key).get(5, TimeUnit.SECONDS);
                log.debug("Resigned from election on {} for participant {}", electionPath, participantId);
            } catch (Exception e) {
                log.warn("Failed to resign from election on {}", electionPath, e);
            }
        }

        // Stop the keep-alive
        CloseableClient kaHandle = keepAliveHandle;
        if (kaHandle != null) {
            try {
                kaHandle.close();
            } catch (Exception e) {
                log.warn("Error closing keep-alive for election on {}", electionPath, e);
            }
        }

        // Revoke the lease to clean up all ephemeral keys
        if (started.get() && leaseId != 0) {
            try {
                client.getLeaseClient().revoke(leaseId).get(5, TimeUnit.SECONDS);
                log.debug("Revoked election lease {} on {}", leaseId, electionPath);
            } catch (Exception e) {
                log.warn("Failed to revoke election lease {} on {}", leaseId, electionPath, e);
            }
        }

        if (isLeaderFlag.getAndSet(false)) {
            notifyListeners(false);
        }

        log.debug("Closed leader election on {} for participant {}", electionPath, participantId);
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

    private static ByteSequence bs(String s) {
        return ByteSequence.from(s, StandardCharsets.UTF_8);
    }

    private static String str(ByteSequence bs) {
        return bs.toString(StandardCharsets.UTF_8);
    }
}
