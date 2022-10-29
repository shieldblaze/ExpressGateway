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
package com.shieldblaze.expressgateway.protocol.http;

import java.util.SplittableRandom;

public class NonceWrapped<T> {

    private static final SplittableRandom RANDOM = new SplittableRandom();

    private final long nonce;
    private final T wrapped;

    public NonceWrapped(T wrapped) {
        this(RANDOM.nextLong(), wrapped);
    }

    public NonceWrapped(long nonce, T wrapped) {
        this.wrapped = wrapped;
        this.nonce = nonce;
    }

    public long nonce() {
        return nonce;
    }

    public T get() {
        return wrapped;
    }

    @Override
    public String toString() {
        return "NonceWrapped{" +
                "nonce=" + nonce +
                ", wrapped=" + wrapped.getClass() +
                '}';
    }
}
