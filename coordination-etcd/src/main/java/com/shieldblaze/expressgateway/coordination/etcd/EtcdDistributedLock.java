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
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.Lease;
import io.etcd.jetcd.Lock;
import io.etcd.jetcd.lease.LeaseKeepAliveResponse;
import io.etcd.jetcd.lock.LockResponse;
import io.etcd.jetcd.support.CloseableClient;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * etcd-backed {@link DistributedLock} using the etcd Lock API (v3lock).
 *
 * <p>Each lock acquisition creates a dedicated lease with keep-alive. The lease ensures
 * the lock is automatically released if this process crashes or loses connectivity.
 * The keep-alive is cancelled and the lease is revoked on {@link #release()}.</p>
 *
 * <p>Thread safety: state is tracked via {@link AtomicBoolean}. Release is idempotent.</p>
 */
@Log4j2
final class EtcdDistributedLock implements DistributedLock {

    private final Client client;
    private final String lockPath;
    private final long leaseId;
    private final ByteSequence lockKey;
    private final CloseableClient keepAliveHandle;
    private final AtomicBoolean held;

    /**
     * Acquires a distributed lock via the etcd Lock API.
     *
     * @param client   the jetcd client
     * @param lockPath the logical lock path
     * @param timeout  maximum time to wait for acquisition
     * @param unit     time unit for the timeout
     * @return a held lock handle
     * @throws CoordinationException if the lock cannot be acquired
     */
    static EtcdDistributedLock acquire(Client client, String lockPath, long timeout, TimeUnit unit)
            throws CoordinationException {
        long leaseId = 0;
        CloseableClient keepAlive = null;

        try {
            Lease leaseClient = client.getLeaseClient();
            Lock lockClient = client.getLockClient();

            // Create a lease with a TTL slightly longer than the timeout to allow for
            // acquisition delay. Minimum 10s to avoid premature expiry.
            long ttlSeconds = Math.max(10, unit.toSeconds(timeout) + 5);
            leaseId = leaseClient.grant(ttlSeconds).get(timeout, unit).getID();

            // Start keep-alive to prevent lease expiry while lock is held.
            // keepAlive() returns a CloseableClient handle that stops the keep-alive stream.
            final long capturedLeaseId = leaseId;
            keepAlive = leaseClient.keepAlive(capturedLeaseId, new StreamObserver<>() {
                @Override
                public void onNext(LeaseKeepAliveResponse response) {
                    log.trace("Lock lease {} keep-alive renewed, TTL={}", capturedLeaseId, response.getTTL());
                }

                @Override
                public void onError(Throwable t) {
                    log.warn("Lock lease {} keep-alive error for {}", capturedLeaseId, lockPath, t);
                }

                @Override
                public void onCompleted() {
                    log.debug("Lock lease {} keep-alive completed for {}", capturedLeaseId, lockPath);
                }
            });

            // Attempt lock acquisition with timeout
            ByteSequence lockName = bs(lockPath);
            CompletableFuture<LockResponse> lockFuture = lockClient.lock(lockName, leaseId);

            LockResponse response;
            try {
                response = lockFuture.get(timeout, unit);
            } catch (TimeoutException e) {
                // Lock acquisition timed out -- clean up lease and keep-alive
                keepAlive.close();
                try {
                    leaseClient.revoke(leaseId).get(5, TimeUnit.SECONDS);
                } catch (Exception suppressed) {
                    log.warn("Failed to revoke lease {} after lock timeout", leaseId, suppressed);
                }
                throw new CoordinationException(CoordinationException.Code.LOCK_ACQUISITION_FAILED,
                        "Failed to acquire lock within " + timeout + " " + unit + ": " + lockPath, e);
            }

            log.debug("Acquired lock on {} with lease {} and key {}",
                    lockPath, leaseId, str(response.getKey()));
            return new EtcdDistributedLock(client, lockPath, leaseId, response.getKey(), keepAlive);

        } catch (CoordinationException e) {
            throw e;
        } catch (ExecutionException e) {
            cleanupOnFailure(client, leaseId, keepAlive, lockPath);
            throw EtcdCoordinationProvider.mapException(e.getCause(), lockPath);
        } catch (InterruptedException e) {
            cleanupOnFailure(client, leaseId, keepAlive, lockPath);
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while acquiring lock: " + lockPath, e);
        } catch (Exception e) {
            cleanupOnFailure(client, leaseId, keepAlive, lockPath);
            throw EtcdCoordinationProvider.mapException(e, lockPath);
        }
    }

    /**
     * Cleans up lease and keep-alive on acquisition failure.
     */
    private static void cleanupOnFailure(Client client, long leaseId,
                                          CloseableClient keepAlive, String lockPath) {
        if (keepAlive != null) {
            try {
                keepAlive.close();
            } catch (Exception e) {
                log.warn("Failed to close keep-alive for lease {} on {}", leaseId, lockPath, e);
            }
        }
        if (leaseId != 0) {
            try {
                client.getLeaseClient().revoke(leaseId).get(5, TimeUnit.SECONDS);
            } catch (Exception e) {
                log.warn("Failed to revoke lease {} on {}", leaseId, lockPath, e);
            }
        }
    }

    private EtcdDistributedLock(Client client, String lockPath, long leaseId,
                                 ByteSequence lockKey, CloseableClient keepAliveHandle) {
        this.client = client;
        this.lockPath = lockPath;
        this.leaseId = leaseId;
        this.lockKey = lockKey;
        this.keepAliveHandle = keepAliveHandle;
        this.held = new AtomicBoolean(true);
    }

    @Override
    public void release() throws CoordinationException {
        if (!held.compareAndSet(true, false)) {
            // Already released -- no-op (idempotent)
            return;
        }

        // Stop the keep-alive first to prevent further renewals
        if (keepAliveHandle != null) {
            try {
                keepAliveHandle.close();
            } catch (Exception e) {
                log.warn("Error closing keep-alive for lock {} lease {}", lockPath, leaseId, e);
            }
        }

        // Unlock the lock key
        try {
            client.getLockClient().unlock(lockKey).get(10, TimeUnit.SECONDS);
            log.debug("Released lock on {}", lockPath);
        } catch (Exception e) {
            log.warn("Failed to unlock {} (will revoke lease to force release)", lockPath, e);
        }

        // Revoke the lease to ensure cleanup even if unlock fails
        try {
            client.getLeaseClient().revoke(leaseId).get(10, TimeUnit.SECONDS);
            log.debug("Revoked lease {} for lock {}", leaseId, lockPath);
        } catch (Exception e) {
            log.warn("Failed to revoke lease {} for lock {}", leaseId, lockPath, e);
        }
    }

    @Override
    public boolean isHeld() {
        return held.get();
    }

    private static ByteSequence bs(String s) {
        return ByteSequence.from(s, StandardCharsets.UTF_8);
    }

    private static String str(ByteSequence bs) {
        return bs.toString(StandardCharsets.UTF_8);
    }
}
