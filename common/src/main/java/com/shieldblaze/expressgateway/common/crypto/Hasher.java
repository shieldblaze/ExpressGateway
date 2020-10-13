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
package com.shieldblaze.expressgateway.common.crypto;

import com.shieldblaze.expressgateway.common.utils.Hex;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

public final class Hasher {

    private static final Logger logger = LogManager.getLogger(Hasher.class);

    public enum Algorithm {
        SHA256,
        SHA384
    }

    public static String hash(Algorithm algorithm, byte[] data) {
        try {
            MessageDigest messageDigest = messageDigest(algorithm);
            return Hex.toHexString(messageDigest.digest(data));
        } catch (Exception ex) {
            logger.error("Error Occurred While Hashing", ex);
        }
        return null;
    }

    private static MessageDigest messageDigest(Algorithm algorithm) throws NoSuchAlgorithmException {
        switch (algorithm) {
            case SHA256:
                return MessageDigest.getInstance("SHA-256");
            case SHA384:
                return MessageDigest.getInstance("SHA-384");
            default:
                throw new IllegalArgumentException("Unknown Algorithm: " + algorithm);
        }
    }
}
