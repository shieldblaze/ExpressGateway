/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.concurrent.event;

import java.util.concurrent.CompletableFuture;

public interface Event<T> {

    /**
     * {@link CompletableFuture} of the operation.
     */
    CompletableFuture<T> future();

    /**
     * Set to {@code true} if the event has finished else set to {@code false}.
     */
    boolean isFinished();

    /**
     * Set to {@code true} if the event has finished and operation was successful else
     * set to {@code false}.
     */
    boolean isSuccess();

    /**
     * Returns {@link Throwable} of the event which has finished and operation was not successful
     * due to some error.
     */
    Throwable cause();
}
