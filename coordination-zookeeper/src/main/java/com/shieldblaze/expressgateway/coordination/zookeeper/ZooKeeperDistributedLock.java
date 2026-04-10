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
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import lombok.extern.log4j.Log4j2;
import org.apache.curator.framework.recipes.locks.InterProcessMutex;

/**
 * ZooKeeper-backed {@link DistributedLock} wrapping Curator's {@link InterProcessMutex}.
 *
 * <p>The lock MUST be released on the same thread that acquired it. This is an
 * {@link InterProcessMutex} requirement and is enforced by Curator.</p>
 */
@Log4j2
final class ZooKeeperDistributedLock implements DistributedLock {

    private final InterProcessMutex mutex;
    private final String lockPath;

    ZooKeeperDistributedLock(InterProcessMutex mutex, String lockPath) {
        this.mutex = mutex;
        this.lockPath = lockPath;
    }

    @Override
    public void release() throws CoordinationException {
        if (!mutex.isOwnedByCurrentThread()) {
            // Already released or never held by this thread -- no-op
            return;
        }
        try {
            mutex.release();
            log.debug("Released lock on {}", lockPath);
        } catch (Exception e) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to release lock: " + lockPath, e);
        }
    }

    @Override
    public boolean isHeld() {
        return mutex.isOwnedByCurrentThread();
    }
}
