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
package com.shieldblaze.expressgateway.common.crypto.cryptostore;

import com.shieldblaze.expressgateway.common.annotation.NonNull;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;

/**
 * {@link CryptoEntry} holds {@link PrivateKey} and {@link X509Certificate}s instances
 *
 * @param privateKey   {@link PrivateKey} instance
 * @param certificates {@link X509Certificate}s instances
 */
public record CryptoEntry(PrivateKey privateKey, X509Certificate... certificates) {

    @NonNull
    public CryptoEntry {
    }
}
