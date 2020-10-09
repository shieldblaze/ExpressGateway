package com.shieldblaze.expressgateway.core.utils;

import com.shieldblaze.expressgateway.core.internal.Internal;
import io.netty.channel.Channel;

import java.util.Objects;

/**
 * Provides extra utilities for {@link Channel} objects.
 */
@Internal
public final class ChannelUtils {

    public static void closeChannels(Channel... channels) {
        if (channels == null) {
            return;
        }

        for (Channel channel : channels) {
            if (channel != null && channel.isActive()) {
                channel.close();
            }
        }
    }
}
