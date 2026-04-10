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
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import lombok.extern.log4j.Log4j2;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Consul session-based {@link DistributedLock} implementation.
 *
 * <p>The lock is held by associating a Consul session with a key via
 * {@code acquireLock}. Releasing the lock calls {@code releaseLock} and
 * destroys the session. The session uses lock-delay to prevent rapid
 * lock oscillation after invalidation.</p>
 *
 * <h2>Thread safety</h2>
 * <p>Hold state is tracked with an {@link AtomicBoolean}. Release is idempotent.</p>
 */
@Log4j2
final class ConsulDistributedLock implements DistributedLock {

    private final Consul consul;
    private final String lockPath;
    private final String sessionId;
    private final AtomicBoolean held;

    ConsulDistributedLock(Consul consul, String lockPath, String sessionId) {
        this.consul = consul;
        this.lockPath = lockPath;
        this.sessionId = sessionId;
        this.held = new AtomicBoolean(true);
    }

    @Override
    public void release() throws CoordinationException {
        if (!held.compareAndSet(true, false)) {
            // Already released -- no-op
            return;
        }
        try {
            consul.keyValueClient().releaseLock(lockPath, sessionId);
            log.debug("Released lock on {} (session={})", lockPath, sessionId);
        } catch (ConsulException e) {
            throw ConsulCoordinationProvider.mapException(e, lockPath);
        } finally {
            // Always destroy the session to clean up server-side state
            try {
                consul.sessionClient().destroySession(sessionId);
            } catch (Exception e) {
                log.warn("Failed to destroy session {} after releasing lock {}", sessionId, lockPath, e);
            }
        }
    }

    @Override
    public boolean isHeld() {
        return held.get();
    }
}
