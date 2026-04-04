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
package com.shieldblaze.expressgateway.common.utils;

import io.netty.util.ReferenceCountUtil;
import io.netty.util.ReferenceCounted;
import lombok.AccessLevel;
import lombok.NoArgsConstructor;

/**
 * Provides extra utilities for {@link ReferenceCounted} objects.
 */
@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class ReferenceCountedUtil {

    public static void silentRelease(Object msg) {
        try {
            ReferenceCountUtil.release(msg);
        } catch (Throwable t) {
            // Swallow the throwable
        }
    }

    public static void silentRelease(Object msg, int decrement) {
        try {
            ReferenceCountUtil.release(msg, decrement);
        } catch (Throwable t) {
            // Swallow the throwable
        }
    }

    public static void silentFullRelease(Object msg) {
        try {
            if (msg instanceof ReferenceCounted rc) {
                ReferenceCountUtil.release(rc, rc.refCnt());
            }
        } catch (Throwable t) {
            // Swallow the throwable
        }
    }
}
