/*
 * Copyright 2020 The Netty Project
 *
 * The Netty Project licenses this file to you under the Apache License, version 2.0 (the
 * "License"); you may not use this file except in compliance with the License. You may obtain a
 * copy of the License at:
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software distributed under the License
 * is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
 * or implied. See the License for the specific language governing permissions and limitations under
 * the License.
 */
package io.netty.handler.codec.http2;

import io.netty.handler.codec.TooLongFrameException;
import io.netty.util.internal.UnstableApi;

import static io.netty.util.internal.ObjectUtil.checkNotNull;

/**
 * A skeletal builder implementation of {@link InboundHttp2ToHttpObjectAdapter} and its subtypes.
 */
@UnstableApi
public abstract class AbstractInboundHttp2ToHttpObjectAdapterBuilder<
        T extends InboundHttp2ToHttpObjectAdapter, B extends AbstractInboundHttp2ToHttpObjectAdapterBuilder<T, B>> {

    private final Http2Connection connection;
    private long maxContentLength;
    private boolean validateHttpHeaders;
    private boolean propagateSettings;

    /**
     * Creates a new {@link io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter} builder for the specified {@link Http2Connection}.
     *
     * @param connection the object which will provide connection notification events
     *                   for the current connection
     */
    protected AbstractInboundHttp2ToHttpObjectAdapterBuilder(Http2Connection connection) {
        this.connection = checkNotNull(connection, "connection");
    }

    @SuppressWarnings("unchecked")
    protected final B self() {
        return (B) this;
    }

    /**
     * Returns the {@link Http2Connection}.
     */
    protected Http2Connection connection() {
        return connection;
    }

    /**
     * Returns the maximum length of the message content.
     */
    protected long maxContentLength() {
        return maxContentLength;
    }

    /**
     * Specifies the maximum length of the message content.
     *
     * @param maxContentLength the maximum length of the message content. If the length of the message content
     *                         exceeds this value, a {@link TooLongFrameException} will be raised
     * @return {@link AbstractInboundHttp2ToHttpObjectAdapterBuilder} the builder for the
     * {@link io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter}
     */
    protected B maxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return self();
    }

    /**
     * Return {@code true} if HTTP header validation should be performed.
     */
    protected boolean isValidateHttpHeaders() {
        return validateHttpHeaders;
    }

    /**
     * Specifies whether validation of HTTP headers should be performed.
     *
     * @param validate <ul>
     *                 <li>{@code true} to validate HTTP headers in the http-codec</li>
     *                 <li>{@code false} not to validate HTTP headers in the http-codec</li>
     *                 </ul>
     * @return {@link AbstractInboundHttp2ToHttpObjectAdapterBuilder} the builder for the
     * {@link io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter}
     */
    protected B validateHttpHeaders(boolean validate) {
        validateHttpHeaders = validate;
        return self();
    }

    /**
     * Returns {@code true} if a read settings frame should be propagated along the channel pipeline.
     */
    protected boolean isPropagateSettings() {
        return propagateSettings;
    }

    /**
     * Specifies whether a read settings frame should be propagated along the channel pipeline.
     *
     * @param propagate if {@code true} read settings will be passed along the pipeline. This can be useful
     *                  to clients that need hold off sending data until they have received the settings.
     * @return {@link AbstractInboundHttp2ToHttpObjectAdapterBuilder} the builder for the
     * {@link io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter}
     */
    protected B propagateSettings(boolean propagate) {
        propagateSettings = propagate;
        return self();
    }

    /**
     * Builds/creates a new {@link io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter} instance using this builder's current settings.
     */
    protected T build() {
        final T instance;
        try {
            instance = build(connection(), maxContentLength(), isValidateHttpHeaders(), isPropagateSettings());
        } catch (Throwable t) {
            throw new IllegalStateException("failed to create a new InboundHttp2ToHttpObjectAdapter", t);
        }
        connection.addListener(instance);
        return instance;
    }

    /**
     * Creates a new {@link InboundHttp2ToHttpObjectAdapter} with the specified properties.
     */
    protected abstract T build(Http2Connection connection, long maxContentLength, boolean validateHttpHeaders,
                               boolean propagateSettings) throws Exception;
}
