module Update exposing (update, urlUpdate)

import Messages exposing (..)
import Model exposing (..)
import Navigation as Nav
import Routing exposing (Route(..), parse, toPath)
import Ports


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlChange location ->
            let
                currentRoute =
                    parse location
            in
                urlUpdate { model | route = currentRoute }

        NavigateTo route ->
            ( model, Nav.newUrl <| toPath route )

        -- HandleInput id value ->

        HandleSearchInput value ->
            ( { model | search = value }, Cmd.none )
        HandleTokenInput value ->
            ( { model | newTokenName = value }, Cmd.none )

        FetchUser user ->
            case user of
                Just u ->
                    ( { model | otherUser = Success u }, Cmd.none )
                Nothing ->
                    ( { model | otherUser = Failure "Not found" }, Cmd.none )

        FetchToken token ->
            ( { model | currentToken = Success token }, Cmd.none )
        CreateToken -> -- TODO: refresh model.newTokenName
            if String.isEmpty model.newTokenName then
                ( model, Cmd.none )
            else
                ( { model | currentToken = Requesting }
                , Cmd.batch
                      [ Ports.createToken model.newTokenName
                      , Ports.fetchToken ()
                      ]
                )
        RevokeToken id ->
            ( { model | currentToken = Requesting }
            , Cmd.batch
                  [ Ports.deleteToken id
                  , Ports.fetchToken ()
                  ]
            )

        LoginOrSignup ->
            ( model, Ports.login () )
        Login user ->
            ( { model | loginUser = Success user }, Cmd.none )
        Logout ->
            ( model, Ports.logout () ) -- TODO: reloadするとか，何かしらのアクションが欲しい．



urlUpdate : Model -> ( Model, Cmd Msg )
urlUpdate model =
    case model.route of
        UsersRoute id ->
            case model.otherUser of
                NotRequested ->
                    ( { model | otherUser = Requesting }
                    , Ports.fetchUser id
                    )
                Success user ->
                    if user.id /= id then
                        ( { model | otherUser = Requesting }
                        , Ports.fetchUser id
                        )
                    else
                        ( model, Cmd.none )
                _ ->
                    ( model, Cmd.none )

        SettingRoute ->
            case (model.loginUser, model.currentToken) of
                (NotRequested, _) -> -- Auto login
                    ( model, Ports.login () )
                (Success _, NotRequested) ->
                    ( { model | currentToken = Requesting }
                    , Ports.fetchToken ()
                    )
                _ ->
                    ( model, Cmd.none )

        _ ->
            case model.loginUser of
                NotRequested ->
                    ( model, Cmd.none )
                _ ->
                    ( model, Cmd.none )
