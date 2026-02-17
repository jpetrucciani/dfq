ARG VERSION=3.22

FROM alpine:${VERSION}

LABEL VERSION="$VERSION"

ARG USER=user
ENV HOME /home/$USER
ENV USER=$USER

# install sudo as root
RUN apk add --update sudo bash curl nano xz

# add new user
RUN adduser -D $USER && \
    echo "$USER ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/$USER && \
    chmod 0440 /etc/sudoers.d/$USER

USER $USER
WORKDIR $HOME

CMD [ "/bin/bash" ]
