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
package com.shieldblaze.expressgateway.common.crypto;

import com.shieldblaze.expressgateway.common.utils.Hex;
import lombok.AccessLevel;
import lombok.NoArgsConstructor;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class Hasher {

    public enum Algorithm {
        SHA256,
        SHA384
    }

    /**
     * Compute a hash of the given data using the specified algorithm.
     *
     * @param algorithm the hash algorithm to use
     * @param data      the data to hash
     * @return the hex-encoded hash string
     * @throws IllegalStateException if the hash algorithm is unavailable
     */
    public static String hash(Algorithm algorithm, byte[] data) {
        try {
            MessageDigest messageDigest = messageDigest(algorithm);
            return Hex.hexString(messageDigest.digest(data));
        } catch (Exception ex) {
            throw new IllegalStateException("Failed to compute hash with algorithm " + algorithm, ex);
        }
    }

    private static MessageDigest messageDigest(Algorithm algorithm) throws NoSuchAlgorithmException {
        return switch (algorithm) {
            case SHA256 -> MessageDigest.getInstance("SHA-256");
            case SHA384 -> MessageDigest.getInstance("SHA-384");
        };
    }
}
