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

/**
 * Generic implementation of {@link Event}
 */
public class GenericEvent<T> implements Event<T> {

    private CompletableFuture<T> future = new CompletableFuture<>();
    private boolean hasFinished;
    private boolean isSuccessful;
    private Throwable throwable;

    public void trySuccess(T object) {
        hasFinished(true);
        isSuccessful(true);
        throwable(null);
        future.complete(object);
    }

    public void tryFailure(Throwable cause) {
        hasFinished(true);
        isSuccessful(false);
        throwable(cause);
        future.completeExceptionally(cause);
    }

    public void future(CompletableFuture<T> future) {
        this.future = future;
    }

    public void hasFinished(boolean finished) {
        this.hasFinished = finished;
    }

    public void isSuccessful(boolean success) {
        this.isSuccessful = success;
    }

    public void throwable(Throwable throwable) {
        this.throwable = throwable;
    }

    @Override
    public CompletableFuture<T> future() {
        return future;
    }

    @Override
    public boolean hasFinished() {
        return hasFinished;
    }

    @Override
    public boolean isSuccessful() {
        return isSuccessful;
    }

    @Override
    public Throwable throwable() {
        return throwable;
    }
}
