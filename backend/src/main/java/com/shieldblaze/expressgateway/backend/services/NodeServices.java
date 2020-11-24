/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.services;

import java.util.concurrent.ScheduledFuture;

final class NodeServices {
    private final ScheduledFuture<?> healthCheck;
    private final ScheduledFuture<?> connectionCleaner;

    NodeServices(ScheduledFuture<?> healthCheck, ScheduledFuture<?> connectionCleaner) {
        this.healthCheck = healthCheck;
        this.connectionCleaner = connectionCleaner;
    }

    ScheduledFuture<?> healthCheck() {
        return healthCheck;
    }

    ScheduledFuture<?> connectionCleaner() {
        return connectionCleaner;
    }
}
