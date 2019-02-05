module Update exposing (update, loadCurrentPage)

import Browser
import Messages exposing (..)
import Model exposing (..)
import Browser.Navigation as Nav
import Ports
import Routing exposing (Route(..))
import Url
--import Scroll


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        OnUrlRequest urlRequest ->
            case urlRequest of
                Browser.Internal url ->
                    ( model
                    , Nav.pushUrl model.navKey (Url.toString url)
                    )

                Browser.External url ->
                    ( model
                    , Nav.load url
                    )

        OnUrlChange url ->
            let
                newRoute =
                    Routing.parseUrl url
            in
            { model | route = newRoute }
                |> loadCurrentPage

        NavigateTo route ->
            ( model
            , Nav.pushUrl model.navKey (Routing.pathFor route)
            )

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

        CreateToken ->
            -- TODO: refresh model.newTokenName
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

        DeletePackage name version ->
            ( model
            , Ports.deletePackage ( name, version )
            )

        LoginOrSignup ->
            ( model, Ports.signin () )

        Signin (Just user) ->
            ( { model | signinUser = Success user }, Cmd.none )

        Signin Nothing ->
            ( { model | signinUser = Requesting }, Cmd.none )

        Signout ->
            ( model
            , Cmd.batch
                [ Ports.signout ()
                , Nav.reload
                , Nav.pushUrl model.navKey (Routing.pathFor HomeIndexRoute)
                ]
            )

        FetchSigninId (Just signinId) ->
            ( { model | signinId = signinId }, Cmd.none )

        FetchSigninId Nothing ->
            ( { model | signinId = "" }, Cmd.none )

        FetchPackages packages ->
            ( { model | listPackages = Success packages }, Cmd.none )

        FetchDetailedPackage (Just packages) ->
            ( { model | detailedPackage = Success packages }, Cmd.none )

        FetchDetailedPackage Nothing ->
            ( { model | detailedPackage = Failure }, Cmd.none )

--        ScrollHandle move ->
--            case model.route of
--                HomeIndexRoute ->
--                    Scroll.handle
--                        [ update (Fadein GetStart)
--                            |> Scroll.onCrossDown 200
--                        , update (Fadein Section1)
--                            |> Scroll.onCrossDown 600
--                        ]
--                        move
--                        model
--
--                _ ->
--                    ( model, Cmd.none )

        OnWidthHandle width ->
            ( { model | width = width }, Cmd.none )

        Fadein view ->
            let
                asIn =
                    case view of
                        GetStart ->
                            asGetStartIn

                        Section1 ->
                            asSection1In

                newModel =
                    True
                        |> asIn model.isFadein
                        |> asIsFadein model
            in
            ( newModel, Cmd.none )

        OnSearchInput searchInput ->
            ( { model | searchInput = searchInput }, Cmd.none )

        Search key ->
            if key == 13 then
                -- Enter key
                ( model, Nav.pushUrl model.navKey (Routing.pathFor <| PackagesRoute "") )

            else
                ( model, Cmd.none )

        HandleChecked bool ->
            ( { model | isChecked = bool }, Cmd.none )


setSection1 : Bool -> IsFadein -> IsFadein
setSection1 newBool isFadein =
    { isFadein | section1 = newBool }


asSection1In : IsFadein -> Bool -> IsFadein
asSection1In =
    \b a -> setSection1 a b


setGetStart : Bool -> IsFadein -> IsFadein
setGetStart newBool isFadein =
    { isFadein | getStart = newBool }


asGetStartIn : IsFadein -> Bool -> IsFadein
asGetStartIn =
    \b a -> setGetStart a b


setIsFadein : IsFadein -> Model -> Model
setIsFadein newIsFadein model =
    { model | isFadein = newIsFadein }


asIsFadein : Model -> IsFadein -> Model
asIsFadein =
    \b a -> setIsFadein a b


loadCurrentPage : Model -> ( Model, Cmd Msg )
loadCurrentPage model =
    case model.route of
        HomeIndexRoute ->
            ( model, Ports.suggest () )

        UsersRoute id ->
            case model.otherUser of
                NotRequested ->
                    ( { model | otherUser = Requesting }
                    , Cmd.batch
                        [ Ports.fetchUser id
                        , Ports.fetchOwnedPackages id
                        ]
                    )

                Success user ->
                    if user.id /= id then
                        ( { model | otherUser = Requesting }
                        , Cmd.batch
                            [ Ports.fetchUser id
                            , Ports.fetchOwnedPackages id
                            ]
                        )

                    else
                        ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        SettingsRoute mode ->
            case mode of
                "tokens" ->
                    case ( model.signinUser, model.currentToken ) of
                        ( Success user, NotRequested ) ->
                            ( { model | currentToken = Requesting }
                            , Ports.fetchToken ()
                            )

                        _ ->
                            ( model, Cmd.none )

                "packages" ->
                    case model.signinUser of
                        Success _ ->
                            ( { model | listPackages = Requesting }
                            , Ports.fetchSigninUserId ()
                            )

                        _ ->
                            ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        SettingRoute ->
            -- /settings だと、/settings/tokenと同じだから
            case ( model.signinUser, model.currentToken ) of
                ( Success user, NotRequested ) ->
                    ( { model | currentToken = Requesting }
                    , Ports.fetchToken ()
                    )

                _ ->
                    ( model, Cmd.none )

        PackagesRoute name ->
            -- TODO: 新規アクセスの度に，listPackagesを空に？？？Usersにアクセスした後だとおかしくなる．
            if String.isEmpty name then
                ( model, Ports.instantsearch () )

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
                org_and_name =
                    org ++ "/" ++ name
            in
            if String.isEmpty org_and_name then
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
                        , Ports.fetchDetailedPackage org_and_name
                        )

                    Success detailedPackage ->
                        if detailedPackage.name /= org_and_name then
                            ( { model | detailedPackage = Requesting }
                            , Ports.fetchDetailedPackage org_and_name
                            )

                        else
                            ( model, Cmd.none )

                    _ ->
                        ( model, Cmd.none )

        _ ->
            ( model, Cmd.none )
