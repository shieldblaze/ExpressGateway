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
package com.shieldblaze.expressgateway.concurrent.task;

import lombok.ToString;

import java.util.concurrent.CompletableFuture;

/**
 * Default implementation of an {@link Task}
 */
@ToString
public class DefaultTask<T> implements Task<T> {

    private final CompletableFuture<T> future = new CompletableFuture<>();
    private volatile boolean isFinished;
    private volatile boolean isSuccessful;
    private volatile Throwable throwable;
    private volatile T object;

    /**
     * Mark this event as successful with 'null' successfulcompletion object
     */
    public void markSuccess() {
        markSuccess(null);
    }

    /**
     * Mark this event as successful
     *
     * @param object Successful completion object
     */
    public void markSuccess(T object) {
        if (isFinished) {
            throw new IllegalStateException("Event is already finished");
        }

        isFinished = true;
        isSuccessful = true;
        future.complete(object);
        this.object = object;

    }

    /**
     * Mark this event as failure
     *
     * @param cause {@link Throwable} of event failure cause
     */
    public void markFailure(Throwable cause) {
        if (isFinished) {
            throw new IllegalStateException("Event is already finished");
        }

        isFinished = true;
        isSuccessful = false;
        throwable = cause;
        future.completeExceptionally(cause);
    }

    public CompletableFuture<T> future() {
        return future;
    }

    @Override
    public T get() {
        return object;
    }

    @Override
    public boolean isFinished() {
        return isFinished;
    }

    @Override
    public boolean isSuccess() {
        return isSuccessful;
    }

    @Override
    public TaskError taskError() {
        DefaultTaskError error = new DefaultTaskError();
        error.addThrowable(throwable);
        return error;
    }
}
