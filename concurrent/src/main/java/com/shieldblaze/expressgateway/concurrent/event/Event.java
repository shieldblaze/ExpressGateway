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
package com.shieldblaze.expressgateway.concurrent.event;

import java.util.concurrent.CompletableFuture;

/**
 * {@link Event} is an object which is created as a result of an operation.
 *
 * @param <T> Type of the operation result
 */
public interface Event<T> {

    /**
     * {@link CompletableFuture} of the operation.
     */
    CompletableFuture<T> future();

    /**
     * Set to {@code true} if the event has finished else set to {@code false}.
     * </p>
     * Note: This does not mean that the operation was successful. Use {@link #isSuccess()} to check that.
     */
    boolean isFinished();

    /**
     * Set to {@code true} if the event has finished and operation was successful else
     * set to {@code false}.
     * <p>
     * </p>
     * {@link #isFinished()} will always return {@code true} if this method returns {@code true}.
     */
    boolean isSuccess();

    /**
     * Returns {@link Throwable} of the event which has finished and operation was not successful
     * due to some error.
     */
    Throwable cause();
}
