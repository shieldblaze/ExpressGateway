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
 * Default implementation of {@link Event}
 */
public class DefaultEvent<T> implements Event<T> {

    private CompletableFuture<T> future = new CompletableFuture<>();
    private boolean isFinished;
    private boolean isSuccessful;
    private Throwable throwable;

    public void trySuccess(T object) {
        isFinished(true);
        isSuccessful(true);
        throwable(null);
        future.complete(object);
    }

    public void tryFailure(Throwable cause) {
        isFinished(true);
        isSuccessful(false);
        throwable(cause);
        future.completeExceptionally(cause);
    }

    public void future(CompletableFuture<T> future) {
        this.future = future;
    }

    public void isFinished(boolean finished) {
        this.isFinished = finished;
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
    public boolean isFinished() {
        return isFinished;
    }

    @Override
    public boolean isSuccess() {
        return isSuccessful;
    }

    @Override
    public Throwable cause() {
        return throwable;
    }

    @Override
    public String toString() {
        return "DefaultEvent{future=" + future + ", isFinished=" + isFinished + ", isSuccessful=" + isSuccessful + ", throwable=" + throwable + '}';
    }
}
