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

/**
 * Handle for participating in a distributed leader election.
 *
 * <p>The election begins when {@link #start()} is called and terminates when
 * {@link #close()} is called. Closing withdraws this participant from the election
 * and triggers leadership re-election among remaining participants.</p>
 *
 * <p>Implementations MUST be thread-safe. Listener callbacks may fire from
 * backend-internal threads.</p>
 */
public interface LeaderElection extends Closeable {

    /**
     * Starts participating in the election. This method is idempotent: calling it
     * on an already-started election is a no-op.
     *
     * @throws CoordinationException if the election cannot be started
     */
    void start() throws CoordinationException;

    /**
     * Returns whether this participant currently holds leadership.
     *
     * <p>This is a point-in-time snapshot. Leadership can be lost at any moment
     * (e.g. due to session expiry). Use {@link #addListener(LeaderChangeListener)}
     * for real-time notifications.</p>
     *
     * @return {@code true} if this participant is the current leader
     */
    boolean isLeader();

    /**
     * Returns the participant ID of the current leader.
     *
     * @return the current leader's participant ID
     * @throws CoordinationException if the leader cannot be determined (no leader
     *                               elected, connection lost, election not started)
     */
    String currentLeaderId() throws CoordinationException;

    /**
     * Registers a listener that is called when this participant's leadership
     * status changes. Listeners are invoked in registration order.
     *
     * @param listener the listener to add; must not be null
     */
    void addListener(LeaderChangeListener listener);

    /**
     * Listener for leadership state transitions.
     */
    @FunctionalInterface
    interface LeaderChangeListener {

        /**
         * Called when this participant's leadership status changes.
         *
         * <p>Implementations MUST NOT block or throw. Exceptions thrown by
         * listeners are logged and swallowed by the caller.</p>
         *
         * @param isLeader {@code true} if this participant became the leader,
         *                 {@code false} if leadership was lost
         */
        void onLeaderChange(boolean isLeader);
    }
}
