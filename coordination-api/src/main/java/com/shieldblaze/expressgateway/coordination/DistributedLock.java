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
package com.shieldblaze.expressgateway.coordination;

import java.io.Closeable;
import java.io.IOException;

/**
 * Handle for a held distributed lock.
 *
 * <p>Obtained via {@link CoordinationProvider#acquireLock(String, long, java.util.concurrent.TimeUnit)}.
 * The lock is held until {@link #release()} or {@link #close()} is called.
 * {@link #close()} delegates to {@link #release()} for try-with-resources convenience.</p>
 *
 * <p>Implementations MUST be thread-safe. The lock MUST be released on the same thread
 * that acquired it (ZooKeeper InterProcessMutex requirement, and a sane default for
 * all backends).</p>
 */
public interface DistributedLock extends Closeable {

    /**
     * Explicitly releases this lock.
     *
     * <p>Calling this on an already-released lock is a no-op.</p>
     *
     * @throws CoordinationException if the lock cannot be released due to a backend error
     */
    void release() throws CoordinationException;

    /**
     * Returns whether this lock is currently held by the calling thread.
     *
     * @return {@code true} if the lock is held by the current thread
     */
    boolean isHeld();

    /**
     * Delegates to {@link #release()} for try-with-resources support.
     *
     * @throws IOException wrapping any {@link CoordinationException} from release
     */
    @Override
    default void close() throws IOException {
        try {
            release();
        } catch (CoordinationException e) {
            throw new IOException("Failed to release distributed lock", e);
        }
    }
}
