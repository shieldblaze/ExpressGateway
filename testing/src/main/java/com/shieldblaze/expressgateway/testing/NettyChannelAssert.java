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
package com.shieldblaze.expressgateway.testing;

import io.netty.channel.Channel;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.embedded.EmbeddedChannel;

import java.util.Objects;

/**
 * Fluent assertion helper for Netty {@link Channel} and {@link EmbeddedChannel} instances.
 * Designed for use in JUnit 5 tests to reduce boilerplate when verifying channel state.
 *
 * <p>Example:</p>
 * <pre>
 *   NettyChannelAssert.assertThat(channel)
 *       .isActive()
 *       .hasHandler("ssl")
 *       .hasHandlerOfType(HttpServerCodec.class);
 * </pre>
 */
public final class NettyChannelAssert {

    private final Channel channel;

    private NettyChannelAssert(Channel channel) {
        this.channel = Objects.requireNonNull(channel, "channel must not be null");
    }

    /**
     * Start a fluent assertion chain for the given channel.
     */
    public static NettyChannelAssert assertThat(Channel channel) {
        return new NettyChannelAssert(channel);
    }

    /**
     * Assert the channel is active.
     */
    public NettyChannelAssert isActive() {
        if (!channel.isActive()) {
            throw new AssertionError("Expected channel to be active but it was inactive");
        }
        return this;
    }

    /**
     * Assert the channel is inactive.
     */
    public NettyChannelAssert isInactive() {
        if (channel.isActive()) {
            throw new AssertionError("Expected channel to be inactive but it was active");
        }
        return this;
    }

    /**
     * Assert the channel is open.
     */
    public NettyChannelAssert isOpen() {
        if (!channel.isOpen()) {
            throw new AssertionError("Expected channel to be open but it was closed");
        }
        return this;
    }

    /**
     * Assert the channel is closed.
     */
    public NettyChannelAssert isClosed() {
        if (channel.isOpen()) {
            throw new AssertionError("Expected channel to be closed but it was open");
        }
        return this;
    }

    /**
     * Assert the channel is writable.
     */
    public NettyChannelAssert isWritable() {
        if (!channel.isWritable()) {
            throw new AssertionError("Expected channel to be writable but it was not");
        }
        return this;
    }

    /**
     * Assert that the pipeline contains a handler with the given name.
     */
    public NettyChannelAssert hasHandler(String name) {
        ChannelPipeline pipeline = channel.pipeline();
        if (pipeline.get(name) == null) {
            throw new AssertionError("Expected pipeline to contain handler '" + name +
                    "' but it was not found. Handlers: " + pipeline.names());
        }
        return this;
    }

    /**
     * Assert that the pipeline does NOT contain a handler with the given name.
     */
    public NettyChannelAssert doesNotHaveHandler(String name) {
        ChannelPipeline pipeline = channel.pipeline();
        if (pipeline.get(name) != null) {
            throw new AssertionError("Expected pipeline NOT to contain handler '" + name +
                    "' but it was found. Handlers: " + pipeline.names());
        }
        return this;
    }

    /**
     * Assert that the pipeline contains at least one handler of the given type.
     */
    public NettyChannelAssert hasHandlerOfType(Class<? extends ChannelHandler> type) {
        ChannelPipeline pipeline = channel.pipeline();
        if (pipeline.get(type) == null) {
            throw new AssertionError("Expected pipeline to contain a handler of type " +
                    type.getSimpleName() + " but none was found. Handlers: " + pipeline.names());
        }
        return this;
    }

    /**
     * Assert the pipeline has exactly the given number of handlers (excluding tail/head contexts).
     */
    public NettyChannelAssert hasHandlerCount(int expectedCount) {
        // pipeline.names() includes "DefaultChannelPipeline$TailContext#0" etc. for non-embedded
        // For EmbeddedChannel the count is straightforward.
        int actual = channel.pipeline().names().size();
        if (actual != expectedCount) {
            throw new AssertionError("Expected " + expectedCount + " handlers but found " +
                    actual + ". Handlers: " + channel.pipeline().names());
        }
        return this;
    }

    /**
     * For EmbeddedChannel: assert that the outbound queue has at least one message.
     */
    public NettyChannelAssert hasOutboundMessages() {
        if (channel instanceof EmbeddedChannel embedded) {
            if (embedded.outboundMessages().isEmpty()) {
                throw new AssertionError("Expected EmbeddedChannel to have outbound messages but queue was empty");
            }
        } else {
            throw new AssertionError("hasOutboundMessages() can only be used with EmbeddedChannel");
        }
        return this;
    }

    /**
     * For EmbeddedChannel: assert that the inbound queue has at least one message.
     */
    public NettyChannelAssert hasInboundMessages() {
        if (channel instanceof EmbeddedChannel embedded) {
            if (embedded.inboundMessages().isEmpty()) {
                throw new AssertionError("Expected EmbeddedChannel to have inbound messages but queue was empty");
            }
        } else {
            throw new AssertionError("hasInboundMessages() can only be used with EmbeddedChannel");
        }
        return this;
    }

    /**
     * Return the underlying channel for further custom assertions.
     */
    public Channel channel() {
        return channel;
    }
}
