/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2024 ShieldBlaze
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

/**
 * A {@link Task} that is already finished.
 *
 * @param <T> the type of the result
 */
public final class SyncTask<T> implements Task<T> {

    private final T object;
    private final Throwable throwable;

    private SyncTask(T object, Throwable throwable) {
        this.object = object;
        this.throwable = throwable;
    }

    public static <T> SyncTask<T> of(T object) {
        return new SyncTask<>(object, null);
    }

    public static <T> SyncTask<T> of(Throwable cause) {
        return new SyncTask<>(null, cause);
    }

    @Override
    public T get() {
        return object;
    }

    @Override
    public boolean isFinished() {
        return true;
    }

    @Override
    public boolean isSuccess() {
        return throwable == null;
    }

    @Override
    public TaskError taskError() {
        DefaultTaskError error = new DefaultTaskError();
        error.addThrowable(throwable);
        return error;
    }
}
