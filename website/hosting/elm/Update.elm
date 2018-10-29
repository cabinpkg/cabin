module Update exposing (update, urlUpdate)

import Messages exposing (..)
import Model exposing (..)
import Navigation as Nav
import Routing exposing (Route(..), parse, toPath)
import Ports
import Array
import Scroll


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
                    ( { model | otherUser = Failure }, Cmd.none )

        FetchToken token ->
            ( { model | currentToken = Success token }, Cmd.none )
        CreateToken -> -- TODO: refresh model.newTokenName
            if String.isEmpty model.newTokenName then
                ( model, Cmd.none )
            else
                ( { model | currentToken = Requesting }
                , Cmd.batch
                      [ Ports.createToken model.newTokenName
                      , Ports.fetchToken <| getUserIdWithDefault model.signinUser
                      ]
                )
        RevokeToken id ->
            ( { model | currentToken = Requesting }
            , Cmd.batch
                  [ Ports.deleteToken id
                  , Ports.fetchToken <| getUserIdWithDefault model.signinUser
                  ]
            )

        LoginOrSignup ->
            ( model, Ports.signin () )
        Signin user ->
            ( { model | signinUser = Success user }, Cmd.none )
        Signout ->
            ( model
            , Cmd.batch
                [ Ports.signout ()
                , Nav.reload
                ]
            )

        FetchPackages packages ->
            ( { model | listPackages = Success packages }, Cmd.none )

        FetchDetailedPackage (Just packages) ->
            ( { model | detailedPackage = Success packages }, Cmd.none )
        FetchDetailedPackage Nothing ->
            ( { model | detailedPackage = Failure }, Cmd.none )

        ScrollHandle move ->
            case model.route of
                HomeIndexRoute ->
                    Scroll.handle
                        [ update (Fadein Abstract)
                            |> Scroll.onCrossDown 200
                            -- when scrollTop > 200px
                        , update (Fadein Section1)
                            |> Scroll.onCrossDown 600
                        , update (Fadein Demo)
                            |> Scroll.onCrossDown 1100
                        , update (Fadein GetStart)
                            |> Scroll.onCrossDown 1600
                        ]
                        move model
                _ ->
                    ( model, Cmd.none )

        Fadein view ->
            let
                asIn =
                    case view of
                        Abstract ->
                            asAbstractIn
                        Section1 ->
                            asSection1In
                        Demo ->
                            asDemoIn
                        GetStart ->
                            asGetStartIn
                newModel =
                    True
                    |> asIn model.isFadein
                    |> asIsFadein model
            in
                ( newModel, Cmd.none )


setAbstract : Bool -> IsFadein -> IsFadein
setAbstract newBool isFadein =
    { isFadein | abstract = newBool }
asAbstractIn : IsFadein -> Bool -> IsFadein
asAbstractIn =
    flip setAbstract

setSection1 : Bool -> IsFadein -> IsFadein
setSection1 newBool isFadein =
    { isFadein | section1 = newBool }
asSection1In : IsFadein -> Bool -> IsFadein
asSection1In =
    flip setSection1

setDemo : Bool -> IsFadein -> IsFadein
setDemo newBool isFadein =
    { isFadein | demo = newBool }
asDemoIn : IsFadein -> Bool -> IsFadein
asDemoIn =
    flip setDemo

setGetStart : Bool -> IsFadein -> IsFadein
setGetStart newBool isFadein =
    { isFadein | getStart = newBool }
asGetStartIn : IsFadein -> Bool -> IsFadein
asGetStartIn =
    flip setGetStart

setIsFadein : IsFadein -> Model -> Model
setIsFadein newIsFadein model =
    { model | isFadein = newIsFadein }
asIsFadein : Model -> IsFadein -> Model
asIsFadein =
    flip setIsFadein


getUserIdWithDefault : RemoteData User -> String
getUserIdWithDefault user =
    user
    |> getUserId
    |> Maybe.withDefault ""

getUserId : RemoteData User -> Maybe String
getUserId user =
    case user of
        Success u ->
            Just u.id
        _ ->
            Nothing


urlUpdate : Model -> ( Model, Cmd Msg )
urlUpdate model =
    case model.route of
        UsersRoute id ->
            case model.otherUser of
                NotRequested ->
                    ( { model | otherUser = Requesting }
                    , Cmd.batch [ Ports.fetchUser id
                                , Ports.fetchOwnedPackages id
                                ]
                    )
                Success user ->
                    if user.id /= id then
                        ( { model | otherUser = Requesting }
                        , Cmd.batch [ Ports.fetchUser id
                                    , Ports.fetchOwnedPackages id
                                    ]
                        )
                    else
                        ( model, Cmd.none )
                _ ->
                    ( model, Cmd.none )

        SettingsRoute mode ->
            case mode of
                "dashboard" ->
                    ( model, Ports.createGraph () )
                "tokens" ->
                    case (model.signinUser, model.currentToken) of
                        (Success user, NotRequested) ->
                            ( { model | currentToken = Requesting }
                            , Ports.fetchToken user.id
                            )
                        _ ->
                            ( model, Cmd.none )
                _ ->
                    ( model, Cmd.none )
        SettingRoute -> -- /settings だと、/settings/tokenと同じだから
            case (model.signinUser, model.currentToken) of
                (Success user, NotRequested) ->
                    ( { model | currentToken = Requesting }
                    , Ports.fetchToken user.id
                    )
                _ ->
                    ( model, Cmd.none )

        PackagesRoute name ->
            if String.isEmpty name then
                case model.listPackages of
                    NotRequested ->
                        ( { model | listPackages = Requesting }
                        , Ports.fetchPackages ()
                        )
                    _ ->
                        ( model, Cmd.none )
            else
                case model.detailedPackage of
                    NotRequested ->
                        ( { model | detailedPackage = Requesting }
                        , Ports.fetchDetailedPackage name
                        )
                    Success detailedPackage ->
                        if detailedPackage.name /= name then
                            ( { model | detailedPackage = Requesting }
                            , Ports.fetchDetailedPackage name
                            )
                        else
                            ( model, Cmd.none )
                    _ ->
                        ( model, Cmd.none )

        OrgPackagesRoute org name ->
            let
                oname =
                    org ++ "/" ++ name
            in
                if String.isEmpty oname then
                    case model.listPackages of
                        NotRequested ->
                            ( { model | listPackages = Requesting }
                            , Ports.fetchPackages ()
                            )
                        _ ->
                            ( model, Cmd.none )
                else
                    case model.detailedPackage of
                        NotRequested ->
                            ( { model | detailedPackage = Requesting }
                            , Ports.fetchDetailedPackage oname
                            )
                        Success detailedPackage ->
                            if detailedPackage.name /= oname then
                                ( { model | detailedPackage = Requesting }
                                , Ports.fetchDetailedPackage oname
                                )
                            else
                                ( model, Cmd.none )
                        _ ->
                            ( model, Cmd.none )

        _ ->
            ( model, Cmd.none )
