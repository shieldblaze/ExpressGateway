package com.shieldblaze.expressgateway.protocol.http;

import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FlowController;
import io.netty.handler.codec.http2.Http2LocalFlowController;
import io.netty.handler.codec.http2.Http2RemoteFlowController;
import io.netty.handler.codec.http2.Http2Stream;
import io.netty.handler.codec.http2.Http2StreamVisitor;
import io.netty.util.concurrent.Future;
import io.netty.util.concurrent.Promise;

public final class DelegatingHttp2Connection extends DefaultHttp2Connection {

    private final DefaultHttp2Connection defaultHttp2Connection;
    private final DelegatingEndpoint<Http2LocalFlowController> localEndpoint;
    private final DelegatingEndpoint<Http2RemoteFlowController> remoteEndpoint;

    public DelegatingHttp2Connection(boolean isServer) {
        super(isServer);
        defaultHttp2Connection = new DefaultHttp2Connection(isServer);
        localEndpoint = new DelegatingEndpoint<>(defaultHttp2Connection.local());
        remoteEndpoint = new DelegatingEndpoint<>(defaultHttp2Connection.remote());
    }

    @Override
    public Future<Void> close(Promise<Void> promise) {
        return defaultHttp2Connection.close(promise);
    }

    @Override
    public PropertyKey newKey() {
        return defaultHttp2Connection.newKey();
    }

    @Override
    public void addListener(Listener listener) {
        defaultHttp2Connection.addListener(listener);
    }

    @Override
    public void removeListener(Listener listener) {
        defaultHttp2Connection.removeListener(listener);
    }

    @Override
    public Http2Stream stream(int streamId) {
        return defaultHttp2Connection.stream(streamId);
    }

    @Override
    public boolean streamMayHaveExisted(int streamId) {
        return defaultHttp2Connection.streamMayHaveExisted(streamId);
    }

    @Override
    public Http2Stream connectionStream() {
        return defaultHttp2Connection.connectionStream();
    }

    @Override
    public int numActiveStreams() {
        return defaultHttp2Connection.numActiveStreams();
    }

    @Override
    public Http2Stream forEachActiveStream(Http2StreamVisitor visitor) throws Http2Exception {
        return defaultHttp2Connection.forEachActiveStream(visitor);
    }

    @Override
    public boolean isServer() {
        return defaultHttp2Connection.isServer();
    }

    @Override
    public Endpoint<Http2LocalFlowController> local() {
        return localEndpoint;
    }

    @Override
    public Endpoint<Http2RemoteFlowController> remote() {
        return remoteEndpoint;
    }

    @Override
    public boolean goAwayReceived() {
        return defaultHttp2Connection.goAwayReceived();
    }

    @Override
    public void goAwayReceived(int lastKnownStream, long errorCode, ByteBuf message) throws Http2Exception {
        defaultHttp2Connection.goAwayReceived(lastKnownStream, errorCode, message);
    }

    @Override
    public boolean goAwaySent() {
        return defaultHttp2Connection.goAwaySent();
    }

    @Override
    public boolean goAwaySent(int lastKnownStream, long errorCode, ByteBuf message) throws Http2Exception {
        return defaultHttp2Connection.goAwaySent();
    }

    public static final class DelegatingEndpoint<F extends Http2FlowController> implements Endpoint<F> {

        private final Endpoint<F> endpoint;

        private DelegatingEndpoint(Endpoint<F> endpoint) {
            this.endpoint = endpoint;
        }

        @Override
        public int incrementAndGetNextStreamId() {
            return endpoint.incrementAndGetNextStreamId();
        }

        @Override
        public boolean isValidStreamId(int streamId) {
            return endpoint.isValidStreamId(streamId);
        }

        @Override
        public boolean mayHaveCreatedStream(int streamId) {
            return endpoint.mayHaveCreatedStream(streamId);
        }

        @Override
        public boolean created(Http2Stream stream) {
            return endpoint.created(stream);
        }

        @Override
        public boolean canOpenStream() {
            return endpoint.canOpenStream();
        }

        @Override
        public Http2Stream createStream(int streamId, boolean halfClosed) throws Http2Exception {
            return endpoint.createStream(streamId, halfClosed);
        }

        @Override
        public Http2Stream reservePushStream(int streamId, Http2Stream parent) throws Http2Exception {
            return endpoint.reservePushStream(streamId, parent);
        }

        @Override
        public boolean isServer() {
            return endpoint.isServer();
        }

        @Override
        public void allowPushTo(boolean allow) {
            endpoint.allowPushTo(allow);
        }

        @Override
        public boolean allowPushTo() {
            return endpoint.allowPushTo();
        }

        @Override
        public int numActiveStreams() {
            return endpoint.numActiveStreams();
        }

        @Override
        public int maxActiveStreams() {
            return endpoint.maxActiveStreams();
        }

        @Override
        public void maxActiveStreams(int maxActiveStreams) {
            endpoint.maxActiveStreams(maxActiveStreams);
        }

        @Override
        public int lastStreamCreated() {
            return endpoint.lastStreamCreated();
        }

        @Override
        public int lastStreamKnownByPeer() {
            return endpoint.lastStreamKnownByPeer();
        }

        @Override
        public F flowController() {
            return endpoint.flowController();
        }

        @Override
        public void flowController(F flowController) {
            endpoint.flowController(flowController);
        }

        @Override
        public Endpoint<? extends Http2FlowController> opposite() {
            return endpoint.opposite();
        }
    }
}
